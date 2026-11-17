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

/// Recoge lo que la app pide al terminal: respuestas a consultas, título y campana.
#[derive(Default)]
pub struct Cb {
    reply: Vec<u8>,
    pub title: String,
    bell: bool,
}

impl vt100::Callbacks for Cb {
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
    pub id: usize,
    pub name: String,
    pub agent: bool,
    pub cwd: PathBuf,
    pub parser: vt100::Parser<Cb>,
    pub status: Status,
    /// Terminó o se bloqueó mientras no lo mirabas.
    pub attention: bool,
    pub scroll: usize,
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
            id,
            name,
            agent,
            cwd: cwd.to_path_buf(),
            parser: vt100::Parser::new_with_callbacks(nz(size.0), nz(size.1), scrollback, Cb::default()),
            status: Status::Working,
            attention: false,
            scroll: 0,
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
        let reply = std::mem::take(&mut self.parser.callbacks_mut().reply);
        if !reply.is_empty() {
            self.write(&reply);
        }
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
        let mut s = Session::spawn(1, "t".into(), false, cmd, &std::env::temp_dir(), (24, 80), 100, &[], tx).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && !s.tail(24).contains("hola-kp") {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(Event::Output(_, b)) => s.feed(&b),
                Ok(_) | Err(_) => {}
            }
        }
        assert!(s.tail(24).contains("hola-kp"), "pantalla: {:?}", s.tail(24));
    }

    /// Los agentes instalados con npm en Windows son .cmd junto a un script sin extensión.
    #[cfg(windows)]
    #[test]
    fn spawns_npm_cmd_shim() {
        if resolve("npm").is_none() {
            return; // sin node instalado
        }
        let (tx, rx) = mpsc::channel();
        let mut s = Session::spawn(1, "t".into(), true, "npm --version", &std::env::temp_dir(), (24, 80), 100, &[], tx).unwrap();
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
