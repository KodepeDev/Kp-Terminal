//! Una sesión = un proceso en un PTY + su pantalla emulada (vt100) + su estado de agente.
use crate::Event;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// Sin salida durante este tiempo = el agente dejó de trabajar.
const QUIET: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Working,
    Waiting,
    Idle,
    Exited,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Waiting => "blocked",
            Status::Idle => "idle",
            Status::Exited => "exited",
        }
    }
}

/// Estado puro a partir de la cola de la pantalla: fácil de probar.
pub fn detect(tail: &str, quiet_for: Duration, exited: bool, patterns: &[String]) -> Status {
    if exited {
        return Status::Exited;
    }
    if quiet_for < QUIET {
        return Status::Working;
    }
    let tail = tail.to_lowercase();
    if patterns.iter().any(|p| tail.contains(p.as_str())) { Status::Waiting } else { Status::Idle }
}

/// Marcas de integración de shell (OSC 133), con la línea absoluta donde ocurrieron.
#[derive(Debug, PartialEq)]
pub enum Mark {
    Prompt,
    Done(Option<i32>),
}

/// Un comando y su salida (bloque estilo Warp).
#[derive(Default, Debug)]
pub struct Block {
    /// Línea absoluta del prompt (contando las que ya salieron del historial).
    pub prompt: u64,
    pub cmd: String,
    pub exit: Option<i32>,
    pub started: Option<Instant>,
    pub dur: Option<Duration>,
}

impl Block {
    pub fn running(&self) -> bool {
        self.started.is_some() && self.dur.is_none()
    }
}

/// Recoge lo que la app pide al terminal: respuestas a consultas, título, campana y marcas de shell.
#[derive(Default)]
pub struct Cb {
    reply: Vec<u8>,
    pub title: String,
    bell: bool,
    marks: Vec<(Mark, u64)>,
    cwd: Option<String>,
    /// Filas descartadas del historial (para numerar líneas de forma absoluta).
    pub dropped: u64,
}

impl vt100::Callbacks for Cb {
    fn on_scroll(&mut self, _: vt100::capture::RowContents<'_>, alternate_screen: bool) {
        if !alternate_screen {
            self.dropped += 1;
        }
    }
    fn unhandled_osc(&mut self, screen: &mut vt100::Screen, params: &[&[u8]]) {
        let abs = self.dropped + screen.scrollback_len() as u64 + screen.cursor_position().0 as u64;
        let text = |i: usize| params.get(i).map(|p| String::from_utf8_lossy(p).to_string());
        match (params.first().copied(), params.get(1).copied()) {
            (Some(b"133"), Some(b"A")) => self.marks.push((Mark::Prompt, abs)),
            (Some(b"133"), Some(b"D")) => {
                self.marks.push((Mark::Done(text(2).and_then(|c| c.trim().parse().ok())), abs))
            }
            // OSC 9;9 (Windows Terminal) y OSC 7 (file://): carpeta actual del shell.
            (Some(b"9"), Some(b"9")) => self.cwd = Some(params[2..].iter().map(|p| String::from_utf8_lossy(p)).collect::<Vec<_>>().join(";")),
            (Some(b"7"), _) => self.cwd = text(1).and_then(|u| crate::shell::file_url_path(&u)),
            _ => {}
        }
    }
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).trim().to_string();
    }
    fn unhandled_csi(&mut self, screen: &mut vt100::Screen, i1: Option<u8>, _: Option<u8>, params: &[&[u16]], c: char) {
        // ConPTY (y muchas TUIs) bloquean esperando estas respuestas.
        let p = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, p) {
            (None, 'n', 6) => {
                let (r, col) = screen.cursor_position();
                self.reply.extend(format!("\x1b[{};{}R", r + 1, col + 1).bytes());
            }
            (None, 'n', 5) => self.reply.extend(b"\x1b[0n"),
            (None, 'c', 0) => self.reply.extend(b"\x1b[?62;22c"),
            (Some(b'>'), 'c', 0) => self.reply.extend(b"\x1b[>0;0;0c"),
            _ => {}
        }
    }
}

pub struct Session {
    /// Bloques de comandos (solo shells con integración).
    pub blocks: Vec<Block>,
    /// El shell muestra su prompt y espera un comando (el editor de kp está activo).
    pub at_prompt: bool,
    /// Se recibieron marcas de integración: hay bloques y editor.
    pub integrated: bool,
    pub editor: crate::editor::Editor,
    size: (u16, u16),
    seen_lines: u64,
    pub id: usize,
    pub name: String,
    pub agent: bool,
    pub cwd: PathBuf,
    pub parser: vt100::Parser<Cb>,
    pub status: Status,
    /// Terminó o se bloqueó mientras no lo mirabas.
    pub attention: bool,
    pub scroll: usize,
    /// Desde cuándo está en el estado actual.
    pub since: Instant,
    /// Rama git de su carpeta (se relee cada pocos segundos).
    pub branch: Option<String>,
    branch_at: Instant,
    last_output: Instant,
    exited: bool,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

/// Resuelve un programa como lo haría cmd.exe: PATH + PATHEXT, ignorando los scripts sin extensión
/// que npm deja junto a los .cmd. Devuelve el argv inicial (los .cmd/.bat van vía `cmd.exe /C`).
#[cfg(windows)]
fn resolve(prog: &str) -> Option<Vec<String>> {
    let wrap = |p: &Path| {
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let s = p.to_string_lossy().to_string();
        if ext == "cmd" || ext == "bat" { vec!["cmd.exe".into(), "/C".into(), s] } else { vec![s] }
    };
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    // Con extensión explícita se busca el nombre tal cual; sin ella, cada extensión de PATHEXT.
    let exts: Vec<&str> = if Path::new(prog).extension().is_some() { vec![""] } else { exts.split(';').filter(|e| !e.is_empty()).collect() };
    let dirs = std::iter::once(PathBuf::new()).chain(std::env::split_paths(&std::env::var_os("PATH")?).collect::<Vec<_>>());
    for dir in dirs {
        for ext in &exts {
            let cand = dir.join(format!("{prog}{ext}"));
            if cand.is_file() {
                return Some(wrap(&cand));
            }
        }
    }
    None
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).unwrap()
}

fn pty_size((rows, cols): (u16, u16)) -> PtySize {
    PtySize { rows: rows.max(1), cols: cols.max(1), pixel_width: 0, pixel_height: 0 }
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        id: usize,
        name: String,
        agent: bool,
        cmd: &str,
        cwd: &Path,
        size: (u16, u16),
        scrollback: usize,
        env: &[(&str, String)],
        integrate: bool,
        tx: Sender<Event>,
    ) -> Result<Session, String> {
        let err = |e: &dyn std::fmt::Display| format!("{name}: {e}");
        let pty = native_pty_system().openpty(pty_size(size)).map_err(|e| err(&e))?;
        let mut argv = cmd.split_whitespace();
        let prog = argv.next().ok_or_else(|| err(&"comando vacío"))?;
        #[cfg(windows)]
        let head = resolve(prog).ok_or_else(|| err(&format!("'{prog}' no está en el PATH")))?;
        #[cfg(not(windows))]
        let head = vec![prog.to_string()];
        let mut cb = CommandBuilder::new(&head[0]);
        cb.args(&head[1..]);
        cb.args(argv);
        if integrate {
            let (args, vars) = crate::shell::integration(prog);
            cb.args(args);
            for (k, v) in vars {
                cb.env(k, v);
            }
        }
        cb.cwd(cwd);
        cb.env("TERM", "xterm-256color");
        cb.env("COLORTERM", "truecolor");
        for (k, v) in env {
            cb.env(k, v);
        }
        let mut child = pty.slave.spawn_command(cb).map_err(|e| err(&format!("no se pudo lanzar '{cmd}': {e}")))?;
        drop(pty.slave);
        let killer = child.clone_killer();
        let mut reader = pty.master.try_clone_reader().map_err(|e| err(&e))?;
        let writer = pty.master.take_writer().map_err(|e| err(&e))?;

        let out = tx.clone();
        // ponytail: 2 hilos por sesión (lector + espera); un poll/IOCP compartido si llegan a cientos de sesiones.
        std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(move || {
                let mut buf = vec![0u8; 32 * 1024];
                while let Ok(n @ 1..) = reader.read(&mut buf) {
                    if out.send(Event::Output(id, buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| err(&e))?;
        std::thread::Builder::new()
            .stack_size(32 * 1024)
            .spawn(move || {
                let _ = child.wait();
                let _ = tx.send(Event::Exit(id));
            })
            .map_err(|e| err(&e))?;

        Ok(Session {
            blocks: vec![],
            at_prompt: false,
            integrated: false,
            editor: Default::default(),
            size,
            seen_lines: 0,
            id,
            name,
            agent,
            cwd: cwd.to_path_buf(),
            parser: vt100::Parser::new_with_callbacks(nz(size.0), nz(size.1), scrollback, Cb::default()),
            status: Status::Working,
            attention: false,
            scroll: 0,
            since: Instant::now(),
            branch: crate::git::branch(cwd),
            branch_at: Instant::now(),
            last_output: Instant::now(),
            exited: false,
            writer,
            master: pty.master,
            killer,
        })
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.last_output = Instant::now();
        let cb = self.parser.callbacks_mut();
        let reply = std::mem::take(&mut cb.reply);
        let marks = std::mem::take(&mut cb.marks);
        let cwd = cb.cwd.take();
        if !reply.is_empty() {
            self.write(&reply);
        }
        // Historial borrado (cls / clear): los bloques anteriores ya no apuntan a nada.
        let lines = self.lines_above();
        if lines < self.seen_lines {
            self.blocks.clear();
        }
        self.seen_lines = lines;
        let dropped = self.parser.callbacks().dropped;
        for (mark, abs) in marks {
            match mark {
                Mark::Prompt => {
                    self.integrated = true;
                    self.at_prompt = true;
                    // Prompts vacíos (Enter sin comando) y bloques "del futuro" (tras borrar pantalla) sobran.
                    self.blocks.retain(|b| b.prompt < abs && b.prompt >= dropped && !b.cmd.is_empty());
                    self.blocks.push(Block { prompt: abs, ..Default::default() });
                    if self.blocks.len() > 500 {
                        self.blocks.remove(0);
                    }
                }
                Mark::Done(code) => {
                    if let Some(b) = self.blocks.iter_mut().rev().find(|b| b.running()) {
                        b.exit = code;
                        b.dur = b.started.map(|t| t.elapsed());
                    } else if let Some(i) = self.blocks.len().checked_sub(1).filter(|&i| {
                        // Comando tecleado directamente en el shell (sin el editor): si produjo salida,
                        // también es un bloque; el comando es la línea del prompt y la duración se desconoce.
                        let b = &self.blocks[i];
                        b.cmd.is_empty() && abs > b.prompt + 1
                    }) {
                        let line = self.line_text(self.blocks[i].prompt);
                        let b = &mut self.blocks[i];
                        b.cmd = line;
                        b.exit = code;
                    }
                }
            }
        }
        if let Some(dir) = cwd.map(PathBuf::from).filter(|d| d.is_dir()) {
            self.cwd = dir;
        }
    }

    /// Filas por encima de la pantalla visible: descartadas + historial.
    fn lines_above(&self) -> u64 {
        self.parser.callbacks().dropped + self.parser.screen().scrollback_len() as u64
    }

    /// Línea absoluta de la fila superior que se está mostrando.
    pub fn top_line(&self) -> u64 {
        self.lines_above() - self.scroll as u64
    }

    /// Bloque al que pertenece una línea absoluta.
    pub fn block_at(&self, line: u64) -> Option<usize> {
        self.blocks.partition_point(|b| b.prompt <= line).checked_sub(1)
    }

    /// Primera línea después del bloque `i`.
    fn block_end(&self, i: usize) -> u64 {
        self.blocks.get(i + 1).map_or_else(
            || self.lines_above() + self.parser.screen().cursor_position().0 as u64 + 1,
            |b| b.prompt,
        )
    }

    /// Texto del bloque `i`: con la línea del prompt+comando o solo la salida.
    pub fn block_text(&mut self, i: usize, with_cmd: bool) -> String {
        let Some(b) = self.blocks.get(i) else { return String::new() };
        let from = b.prompt + u64::from(!with_cmd);
        let to = self.block_end(i);
        let mut lines: Vec<String> = (from..to).map(|l| self.line_text(l)).collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Texto de una línea absoluta (vacío si ya salió del historial).
    fn line_text(&mut self, line: u64) -> String {
        let (dropped, sb) = (self.parser.callbacks().dropped, self.parser.screen().scrollback_len() as u64);
        let Some(rel) = line.checked_sub(dropped) else { return String::new() };
        // Las filas del historial se leen moviendo la vista temporalmente.
        let (offset, row) = if rel < sb { ((sb - rel) as usize, 0) } else { (0, (rel - sb) as usize) };
        self.parser.screen_mut().set_scrollback(offset);
        let text = self.parser.screen().rows(0, self.size.1).nth(row).unwrap_or_default().trim_end().to_string();
        self.parser.screen_mut().set_scrollback(self.scroll);
        text
    }

    /// Desplaza la vista para que la línea absoluta `line` quede arriba si no está visible.
    pub fn reveal(&mut self, line: u64) {
        let top = self.top_line();
        if line < top || line >= top + self.size.0 as u64 {
            let want = self.lines_above().saturating_sub(line) as i64;
            self.scroll_by((want - self.scroll as i64) as i32);
        }
    }

    /// El editor de kp recibe las teclas: shell integrado esperando en su prompt.
    pub fn editor_active(&self) -> bool {
        self.integrated && self.at_prompt && !self.exited && !self.parser.screen().alternate_screen()
    }

    /// Ejecuta un comando desde el editor y abre su bloque.
    pub fn submit(&mut self, cmd: &str) {
        if let Some(b) = self.blocks.last_mut().filter(|b| b.cmd.is_empty()) {
            b.cmd = cmd.trim().to_string();
            b.started = Some(Instant::now());
        }
        self.at_prompt = false;
        if self.scroll > 0 {
            self.scroll_by(-(self.scroll as i32));
        }
        // ponytail: un comando multilínea llega como varias líneas; PSReadLine sin bracketed paste las ejecuta por separado.
        self.paste(cmd);
        self.write(b"\r");
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if !self.exited {
            let _ = self.writer.write_all(bytes);
            let _ = self.writer.flush();
        }
    }

    /// Pega texto respetando bracketed paste si la app lo activó.
    pub fn paste(&mut self, text: &str) {
        if self.parser.screen().bracketed_paste() {
            let wrapped = format!("\x1b[200~{text}\x1b[201~");
            self.write(wrapped.as_bytes());
        } else {
            self.write(text.as_bytes());
        }
    }

    /// Escribe un prompt y lo envía (Enter aparte para que no quede dentro del pegado).
    pub fn send_prompt(&mut self, text: &str) {
        self.paste(text.trim_end());
        self.write(b"\r");
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if self.size == (rows, cols) {
            return;
        }
        self.size = (rows, cols);
        self.parser.set_size(nz(rows), nz(cols));
        let _ = self.master.resize(pty_size((rows, cols)));
    }

    pub fn scroll_by(&mut self, delta: i32) {
        let want = (self.scroll as i64 + delta as i64).max(0) as usize;
        self.parser.screen_mut().set_scrollback(want);
        self.scroll = self.parser.screen().scrollback();
    }

    pub fn title(&self) -> &str {
        let t = &self.parser.callbacks().title;
        // El título por defecto de ConPTY/cmd es la ruta del .exe: no aporta nada.
        if t.to_lowercase().ends_with(".exe") { "" } else { t }
    }

    /// Últimas `n` líneas con contenido de la pantalla visible.
    pub fn tail(&self, n: usize) -> String {
        let text = self.parser.screen().contents();
        let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
        let end = lines.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
        lines[end.saturating_sub(n)..end].join("\n")
    }

    /// Recalcula el estado. Devuelve (cambió, debe_sonar).
    pub fn tick(&mut self, patterns: &[String], visible: bool) -> (bool, bool) {
        let old = self.status;
        // ponytail: si el usuario está con scroll hacia arriba, la cola mira el historial; aceptable.
        self.status = detect(&self.tail(12), self.last_output.elapsed(), self.exited, patterns);
        let bell = std::mem::take(&mut self.parser.callbacks_mut().bell);
        // "Terminó" solo importa en agentes; un shell pasa a idle en cada prompt.
        let finished = self.agent && old == Status::Working && self.status != Status::Working;
        let ring = !visible && (finished || bell);
        if ring {
            self.attention = true;
        }
        if visible {
            self.attention = false;
        }
        if old != self.status {
            self.since = Instant::now();
        }
        if self.branch_at.elapsed() > Duration::from_secs(3) {
            self.branch_at = Instant::now();
            self.branch = crate::git::branch(&self.cwd);
        }
        (old != self.status || ring, ring)
    }

    pub fn mark_exited(&mut self) {
        self.exited = true;
    }

    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn detects_status() {
        let pats: Vec<String> = crate::config::Config::default().waiting_patterns;
        let long = Duration::from_secs(5);
        assert_eq!(detect("x", Duration::ZERO, false, &pats), Status::Working);
        assert_eq!(detect("Do you want to proceed?\n❯ 1. Yes", long, false, &pats), Status::Waiting);
        assert_eq!(detect("Overwrite? [y/N]", long, false, &pats), Status::Waiting);
        assert_eq!(detect("❯ No, exit\n  Yes, I trust this folder\nEnter to confirm · Esc to cancel", long, false, &pats), Status::Waiting);
        assert_eq!(detect("PS C:\\> ", long, false, &pats), Status::Idle);
        assert_eq!(detect("Do you want", long, true, &pats), Status::Exited);
    }

    /// Camino real PTY -> hilo lector -> canal -> parser vt100.
    #[test]
    fn pty_roundtrip() {
        let (tx, rx) = mpsc::channel();
        let cmd = if cfg!(windows) { "cmd.exe /C echo hola-kp" } else { "echo hola-kp" };
        let mut s = Session::spawn(1, "t".into(), false, cmd, &std::env::temp_dir(), (24, 80), 100, &[], false, tx).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && !s.tail(24).contains("hola-kp") {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(Event::Output(_, b)) => s.feed(&b),
                Ok(_) | Err(_) => {}
            }
        }
        assert!(s.tail(24).contains("hola-kp"), "pantalla: {:?}", s.tail(24));
    }

    /// Integración real con PowerShell: bloques con comando, código de salida y texto.
    #[cfg(windows)]
    #[test]
    fn powershell_blocks() {
        let (tx, rx) = mpsc::channel();
        let mut s = Session::spawn(1, "ps".into(), false, "powershell.exe -NoLogo -NoProfile", &std::env::temp_dir(), (30, 100), 500, &[], true, tx).unwrap();
        let pump = |s: &mut Session, until: &dyn Fn(&Session) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline && !until(s) {
                if let Ok(Event::Output(_, b)) = rx.recv_timeout(Duration::from_millis(100)) {
                    s.feed(&b);
                }
            }
        };
        pump(&mut s, &|s| s.editor_active());
        assert!(s.integrated, "sin marcas OSC 133: {:?}", s.tail(10));
        s.submit("echo kp-bloque");
        pump(&mut s, &|s| s.blocks.first().is_some_and(|b| b.dur.is_some()) && s.editor_active());
        let b = &s.blocks[0];
        assert_eq!((b.cmd.as_str(), b.exit), ("echo kp-bloque", Some(0)));
        assert_eq!(s.block_text(0, false), "kp-bloque");
        assert!(s.block_text(0, true).contains("echo kp-bloque"));

        s.submit("cmd /c exit 3");
        pump(&mut s, &|s| s.blocks.get(1).is_some_and(|b| b.dur.is_some()) && s.editor_active());
        assert_eq!(s.blocks[1].exit, Some(3));
        assert_eq!(s.block_at(s.blocks[1].prompt), Some(1));
        assert!(s.cwd.is_dir());
        s.kill();
    }

    /// Los agentes instalados con npm en Windows son .cmd junto a un script sin extensión.
    #[cfg(windows)]
    #[test]
    fn spawns_npm_cmd_shim() {
        if resolve("npm").is_none() {
            return; // sin node instalado
        }
        let (tx, rx) = mpsc::channel();
        let mut s = Session::spawn(1, "t".into(), true, "npm --version", &std::env::temp_dir(), (24, 80), 100, &[], false, tx).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let has_version = |t: &str| t.lines().any(|l| l.trim().split('.').count() == 3 && l.trim().starts_with(|c: char| c.is_ascii_digit()));
        while Instant::now() < deadline && !has_version(&s.tail(24)) {
            if let Ok(Event::Output(_, b)) = rx.recv_timeout(Duration::from_millis(200)) {
                s.feed(&b);
            }
        }
        assert!(has_version(&s.tail(24)), "pantalla: {:?}", s.tail(24));
    }
}
