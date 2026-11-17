//! Panel de revisión estilo cliente git: cambios, diff coloreado y botones (ratón o teclado).
use crate::Event;
use crate::config::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode as K, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

const MAX_DIFF_LINES: usize = 5000;

/// Ejecuta git en `dir` sin tocar la terminal; devuelve stdout o el stderr como error.
pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "core.quotepath=off", "-c", "color.ui=never"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = if err.trim().is_empty() { String::from_utf8_lossy(&out.stdout) } else { err };
        Err(format!("git {}: {}", args[0], err.trim()))
    }
}

/// Rama actual leyendo `.git/HEAD` (sin lanzar procesos; vale también para worktrees).
pub fn branch(dir: &Path) -> Option<String> {
    for p in dir.ancestors() {
        let g = p.join(".git");
        let gitdir = if g.is_dir() {
            g
        } else if g.is_file() {
            // Worktree: `.git` es un archivo `gitdir: <ruta>`.
            let s = std::fs::read_to_string(&g).ok()?;
            p.join(s.trim().strip_prefix("gitdir:")?.trim())
        } else {
            continue;
        };
        let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
        let head = head.trim();
        return Some(head.strip_prefix("ref: refs/heads/").map_or_else(|| head.chars().take(7).collect(), String::from));
    }
    None
}

/// Crea un worktree hermano del repo en una rama nueva, para que varios agentes trabajen sin pisarse.
pub fn worktree(dir: &Path, preset: &str) -> Result<PathBuf, String> {
    let root = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?.trim());
    let repo = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let path = root.with_file_name(format!("{repo}-{preset}-{stamp}"));
    git(dir, &["worktree", "add", "-b", &format!("kp/{preset}-{stamp}"), &path.to_string_lossy()])?;
    Ok(path)
}

#[derive(Debug, PartialEq)]
pub struct Entry {
    /// Estado en el índice (staged) y en el árbol de trabajo, como `git status --porcelain`.
    pub x: char,
    pub y: char,
    pub path: String,
}

impl Entry {
    fn untracked(&self) -> bool {
        self.x == '?'
    }
}

/// Cabecera `## ...` de git status, resumida: `main ↑1 ↓2`, `main · sin commits`.
pub fn branch_label(raw: &str) -> String {
    if let Some(b) = raw.strip_prefix("No commits yet on ") {
        return format!("{b} · sin commits");
    }
    let name = raw.split("...").next().unwrap_or(raw).split(' ').next().unwrap_or(raw);
    let mut out = name.to_string();
    if let Some(info) = raw.split_once('[').map(|(_, r)| r.trim_end_matches(']')) {
        for part in info.split(", ") {
            if let Some(n) = part.strip_prefix("ahead ") {
                out += &format!(" ↑{n}");
            } else if let Some(n) = part.strip_prefix("behind ") {
                out += &format!(" ↓{n}");
            }
        }
    }
    out
}

/// Parsea `git status --porcelain=v1 -b -z`: (rama, entradas).
pub fn parse_status(s: &str) -> (String, Vec<Entry>) {
    let mut branch = String::new();
    let mut files = vec![];
    let mut it = s.split('\0');
    while let Some(e) = it.next() {
        if let Some(b) = e.strip_prefix("## ") {
            branch = b.to_string();
        } else if e.len() > 3 {
            let b = e.as_bytes();
            let (x, y) = (b[0] as char, b[1] as char);
            if x == 'R' || x == 'C' {
                it.next(); // ruta de origen del rename
            }
            files.push(Entry { x, y, path: e[3..].to_string() });
        }
    }
    (branch, files)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Btn {
    Refresh,
    Stage,
    Unstage,
    StageAll,
    Commit,
    Discard,
    Ask,
    Pull,
    Push,
    Close,
}

const BUTTONS: [(Btn, &str, char); 10] = [
    (Btn::Refresh, "⟳ Actualizar", 'r'),
    (Btn::Stage, "+ Stage", 's'),
    (Btn::Unstage, "− Unstage", 'u'),
    (Btn::StageAll, "++ Todo", 'A'),
    (Btn::Commit, "✓ Commit", 'c'),
    (Btn::Discard, "⎌ Descartar", 'x'),
    (Btn::Ask, "✎ Pedir cambio", 'a'),
    (Btn::Pull, "⇣ Pull", 'p'),
    (Btn::Push, "⇡ Push", 'P'),
    (Btn::Close, "✕ Cerrar", 'q'),
];

#[derive(Clone, Copy, PartialEq)]
enum Input {
    Commit,
    Ask,
}

/// Lo que el panel pide a la app.
pub enum Req {
    Close,
    /// Enviar un prompt de revisión al agente de la sesión revisada.
    Ask(String),
}

pub struct GitView {
    pub dir: PathBuf,
    pub session: Option<usize>,
    branch: String,
    files: Vec<Entry>,
    sel: usize,
    diff: Vec<String>,
    scroll: usize,
    input: Option<(Input, String)>,
    confirm: Option<Btn>,
    pub msg: String,
    busy: bool,
    tx: Sender<Event>,
    // Zonas clicables, calculadas en cada render.
    buttons: Vec<(Rect, Btn)>,
    list: Rect,
    list_first: usize,
    diff_area: Rect,
}

impl GitView {
    pub fn open(dir: &Path, session: Option<usize>, tx: Sender<Event>) -> Result<GitView, String> {
        let root = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?.trim());
        let mut v = GitView {
            dir: root,
            session,
            branch: String::new(),
            files: vec![],
            sel: 0,
            diff: vec![],
            scroll: 0,
            input: None,
            confirm: None,
            msg: String::new(),
            busy: false,
            tx,
            buttons: vec![],
            list: Rect::default(),
            list_first: 0,
            diff_area: Rect::default(),
        };
        v.refresh();
        Ok(v)
    }

    pub fn refresh(&mut self) {
        match git(&self.dir, &["status", "--porcelain=v1", "-b", "-z", "-uall"]) {
            Ok(s) => (self.branch, self.files) = parse_status(&s),
            Err(e) => self.msg = e,
        }
        self.sel = self.sel.min(self.files.len().saturating_sub(1));
        self.load_diff();
    }

    fn current(&self) -> Option<&Entry> {
        self.files.get(self.sel)
    }

    fn load_diff(&mut self) {
        self.scroll = 0;
        self.diff.clear();
        let Some(e) = self.current() else { return };
        let path = e.path.clone();
        if e.untracked() {
            self.diff.push(format!("nuevo archivo: {path}"));
            match std::fs::read(self.dir.join(&path)) {
                Ok(b) if b.contains(&0) => self.diff.push("(binario)".into()),
                Ok(b) => self.diff.extend(String::from_utf8_lossy(&b).lines().take(MAX_DIFF_LINES).map(|l| format!("+{l}"))),
                Err(err) => self.diff.push(err.to_string()),
            }
            return;
        }
        for (label, cached) in [("── staged ──", true), ("── sin stage ──", false)] {
            let mut args = vec!["diff", "--no-ext-diff"];
            if cached {
                args.push("--cached");
            }
            args.extend(["--", &path]);
            match git(&self.dir, &args) {
                Ok(d) if d.trim().is_empty() => {}
                Ok(d) => {
                    self.diff.push(label.into());
                    self.diff.extend(d.lines().map(String::from));
                }
                Err(e) => self.diff.push(e),
            }
        }
        self.diff.truncate(MAX_DIFF_LINES);
    }

    fn select(&mut self, i: usize) {
        if i < self.files.len() && i != self.sel {
            self.sel = i;
            self.load_diff();
        }
    }

    /// Ejecuta una acción local y refresca.
    fn run(&mut self, args: &[&str], ok: &str) {
        match git(&self.dir, args) {
            Ok(_) => self.msg = ok.into(),
            Err(e) => self.msg = e,
        }
        self.refresh();
    }

    pub fn press(&mut self, b: Btn) -> Option<Req> {
        let pending = self.confirm.take();
        self.msg.clear();
        let path = self.current().map(|e| e.path.clone());
        match (b, path.as_deref()) {
            (Btn::Refresh, _) => self.refresh(),
            (Btn::Close, _) => return Some(Req::Close),
            (Btn::StageAll, _) => self.run(&["add", "-A"], "todo en stage"),
            (Btn::Commit, _) => self.input = Some((Input::Commit, String::new())),
            (Btn::Ask, _) if self.session.is_none() => self.msg = "no hay sesión de agente asociada".into(),
            (Btn::Ask, _) => self.input = Some((Input::Ask, String::new())),
            (Btn::Pull | Btn::Push, _) if self.busy => self.msg = "ya hay una operación en curso".into(),
            (Btn::Push, _) if pending != Some(Btn::Push) => {
                // Publica en el remoto: segunda pulsación para confirmar.
                self.confirm = Some(Btn::Push);
                self.msg = format!("¿Push de {}? Pulsa Push otra vez", self.branch);
            }
            (Btn::Pull | Btn::Push, _) => {
                let (dir, tx) = (self.dir.clone(), self.tx.clone());
                let args: &[&str] = if b == Btn::Push { &["push"] } else { &["pull", "--ff-only"] };
                self.busy = true;
                self.msg = format!("git {}…", args[0]);
                // Red: en segundo plano para no congelar la interfaz.
                std::thread::spawn(move || {
                    let r = git(&dir, args).map(|o| format!("git {} ok {}", args[0], o.trim()));
                    let _ = tx.send(Event::GitDone(r.unwrap_or_else(|e| e)));
                });
            }
            (_, None) => self.msg = "no hay archivo seleccionado".into(),
            (Btn::Stage, Some(p)) => self.run(&["add", "--", p], "en stage"),
            (Btn::Unstage, Some(p)) => self.run(&["restore", "--staged", "--", p], "fuera de stage"),
            (Btn::Discard, Some(p)) => {
                // Pérdida de datos: pide confirmar con un segundo clic/tecla.
                if pending != Some(Btn::Discard) {
                    self.confirm = Some(Btn::Discard);
                    self.msg = format!("¿Descartar TODOS los cambios de {p}? Pulsa Descartar otra vez");
                    return None;
                }
                let e = self.current().unwrap();
                let args: Vec<&str> = if e.untracked() {
                    vec!["clean", "-f", "--", p]
                } else if e.x == 'A' {
                    vec!["rm", "-f", "--", p]
                } else {
                    vec!["restore", "--source=HEAD", "--staged", "--worktree", "--", p]
                };
                self.run(&args, "cambios descartados");
            }
        }
        None
    }

    /// Resultado de pull/push en segundo plano.
    pub fn done(&mut self, msg: String) {
        self.busy = false;
        self.refresh();
        self.msg = msg.lines().last().unwrap_or("").to_string();
    }

    pub fn key(&mut self, k: KeyEvent) -> Option<Req> {
        if let Some((_, text)) = &mut self.input {
            match k.code {
                K::Esc => self.input = None,
                K::Backspace => {
                    text.pop();
                }
                K::Char(c) => text.push(c),
                K::Enter => {
                    let (kind, text) = self.input.take().unwrap();
                    let text = text.trim().to_string();
                    if text.is_empty() {
                        return None;
                    }
                    if kind == Input::Commit {
                        self.run(&["commit", "-m", &text], "commit creado");
                    } else {
                        let file = self.current().map_or(String::new(), |e| format!(" sobre `{}`", e.path));
                        self.msg = "comentario enviado al agente".into();
                        return Some(Req::Ask(format!("Revisión de código{file}: {text}")));
                    }
                }
                _ => {}
            }
            return None;
        }
        let page = (self.diff_area.height.max(2) - 1) as usize;
        match k.code {
            K::Esc => return Some(Req::Close),
            K::Up | K::Char('k') => self.select(self.sel.saturating_sub(1)),
            K::Down | K::Char('j') => self.select(self.sel + 1),
            K::PageDown | K::Char(' ') => self.scroll_diff(page as i32),
            K::PageUp | K::Char('b') => self.scroll_diff(-(page as i32)),
            K::Char(c) => {
                if let Some((b, _, _)) = BUTTONS.iter().find(|(_, _, key)| *key == c) {
                    return self.press(*b);
                }
            }
            _ => {}
        }
        None
    }

    /// Pegado: solo va a la entrada de texto activa; nunca al agente por debajo.
    pub fn paste(&mut self, text: &str) {
        if let Some((_, input)) = &mut self.input {
            input.push_str(&text.replace(['\r', '\n'], " "));
        }
    }

    fn scroll_diff(&mut self, d: i32) {
        let max = self.diff.len().saturating_sub(1);
        self.scroll = ((self.scroll as i64 + d as i64).max(0) as usize).min(max);
    }

    pub fn click(&mut self, col: u16, row: u16) -> Option<Req> {
        let hit = |r: &Rect| col >= r.x && col < r.right() && row >= r.y && row < r.bottom();
        if let Some((_, b)) = self.buttons.iter().find(|(r, _)| hit(r)) {
            return self.press(*b);
        }
        if hit(&self.list) {
            self.select(self.list_first + (row - self.list.y) as usize);
        }
        None
    }

    pub fn wheel(&mut self, col: u16, row: u16, up: bool) {
        let r = self.diff_area;
        if col >= r.x && col < r.right() && row >= r.y && row < r.bottom() {
            self.scroll_diff(if up { -3 } else { 3 });
        } else if up {
            self.select(self.sel.saturating_sub(1));
        } else {
            self.select(self.sel + 1);
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let [bar, body, foot] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).areas(area);

        // Barra de botones.
        self.buttons.clear();
        let head = format!(" ⎇ {} ", branch_label(&self.branch));
        let mut spans = vec![Span::styled(head.clone(), Style::new().fg(t.accent).add_modifier(Modifier::BOLD))];
        let mut x = bar.x + head.chars().count() as u16;
        // Si no caben con texto, solo tecla + icono: nunca se esconden botones.
        let full: usize = BUTTONS.iter().map(|(_, l, _)| l.chars().count() + 5).sum();
        let compact = x as usize + full > bar.right() as usize;
        for (b, label, key) in BUTTONS {
            let text = if compact {
                format!(" {key} {} ", label.split(' ').next().unwrap_or(label))
            } else {
                format!(" {key} {label} ")
            };
            let w = text.chars().count() as u16;
            if x + w > bar.right() {
                break;
            }
            let armed = self.confirm == Some(b);
            let bg = if armed { t.waiting } else if b == Btn::Commit { t.accent } else { t.sel_bg };
            let fg = if armed || b == Btn::Commit { t.on_accent } else { t.accent };
            spans.push(Span::styled(text, Style::new().bg(bg).fg(fg).add_modifier(Modifier::BOLD)));
            spans.push(Span::raw(" "));
            self.buttons.push((Rect::new(x, bar.y, w, 1), b));
            x += w + 1;
        }
        f.render_widget(Paragraph::new(Line::from(spans)).style(Style::new().bg(t.bar_bg)), bar);

        // Lista de archivos.
        let [list, diff] = Layout::horizontal([Constraint::Percentage(32), Constraint::Min(10)]).areas(body);
        let staged = self.files.iter().filter(|e| e.x != ' ' && e.x != '?').count();
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(t.dim))
            .title(Span::styled(format!(" Cambios {} · {staged} staged ", self.files.len()), Style::new().fg(t.accent)));
        let inner = block.inner(list);
        f.render_widget(block, list);
        let visible = inner.height as usize;
        let first = self.sel.saturating_sub(visible.saturating_sub(1));
        (self.list, self.list_first) = (inner, first);
        let lines: Vec<Line> = if self.files.is_empty() {
            vec![Line::styled(" sin cambios ✔", Style::new().fg(t.done))]
        } else {
            self.files
                .iter()
                .enumerate()
                .skip(first)
                .take(visible)
                .map(|(i, e)| {
                    let col = |c: char, staged: bool| match c {
                        ' ' => Span::raw(" "),
                        '?' => Span::styled("?", Style::new().fg(t.working)),
                        c => Span::styled(c.to_string(), Style::new().fg(if staged { t.add } else { t.del })),
                    };
                    let bg = if i == self.sel { Style::new().bg(t.sel_bg) } else { Style::new() };
                    Line::from(vec![Span::raw(" "), col(e.x, true), col(e.y, false), Span::raw(" "), Span::raw(e.path.clone())])
                        .style(bg)
                })
                .collect()
        };
        f.render_widget(Paragraph::new(lines), inner);

        // Diff.
        let title = self.current().map_or(String::new(), |e| format!(" {} ", e.path));
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(t.dim))
            .title(Span::styled(title, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
            .title_bottom(Span::styled(
                format!(" {}/{} · j/k archivo · Espacio/b página · rueda ", self.scroll + 1, self.diff.len().max(1)),
                Style::new().fg(t.dim),
            ));
        self.diff_area = block.inner(diff);
        f.render_widget(block, diff);
        let lines: Vec<Line> = self
            .diff
            .iter()
            .skip(self.scroll)
            .take(self.diff_area.height as usize)
            .map(|l| {
                let header = ["+++ a/", "+++ b/", "--- a/", "--- b/", "+++ /dev/null", "--- /dev/null", "diff --git ", "index "];
                let style = if header.iter().any(|h| l.starts_with(h)) {
                    Style::new().fg(t.dim)
                } else if l.starts_with('+') {
                    Style::new().fg(t.add)
                } else if l.starts_with('-') {
                    Style::new().fg(t.del)
                } else if l.starts_with("@@") || l.starts_with("──") {
                    Style::new().fg(t.accent)
                } else {
                    Style::new()
                };
                Line::styled(l.replace('\t', "    "), style)
            })
            .collect();
        f.render_widget(Paragraph::new(lines), self.diff_area);

        // Pie: entrada de texto o mensaje.
        let line = match &self.input {
            Some((kind, text)) => {
                let label = if *kind == Input::Commit { " mensaje de commit " } else { " pedir cambio al agente " };
                Line::from(vec![
                    Span::styled(label, Style::new().bg(t.accent).fg(t.on_accent).add_modifier(Modifier::BOLD)),
                    Span::raw(format!(" {text}█")),
                    Span::styled("   Enter enviar · Esc cancelar", Style::new().fg(t.dim)),
                ])
            }
            None if !self.msg.is_empty() => Line::styled(format!(" {}", self.msg), Style::new().fg(t.working)),
            // Leyenda de teclas (los botones pueden estar en modo compacto) y la carpeta revisada.
            None => {
                let mut spans = vec![];
                for (_, label, key) in BUTTONS {
                    let name = label.split_once(' ').map_or(label, |(_, n)| n).to_lowercase();
                    spans.push(Span::styled(format!(" {key}"), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)));
                    spans.push(Span::styled(format!(" {name} "), Style::new().fg(t.dim)));
                }
                spans.push(Span::styled(format!("  {}", self.dir.display()), Style::new().fg(t.dim)));
                Line::from(spans)
            }
        };
        f.render_widget(Paragraph::new(line).style(Style::new().bg(t.bar_bg)), foot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn parses_status() {
        let s = "## main...origin/main [ahead 1]\0M  src/a.rs\0 M b.txt\0R  new.rs\0old.rs\0?? dir/c d.md\0";
        let (branch, files) = parse_status(s);
        assert_eq!(branch, "main...origin/main [ahead 1]");
        assert_eq!(files.len(), 4);
        assert_eq!(files[0], Entry { x: 'M', y: ' ', path: "src/a.rs".into() });
        assert_eq!(files[2].path, "new.rs");
        assert_eq!(files[3], Entry { x: '?', y: '?', path: "dir/c d.md".into() });
        assert_eq!(branch_label(&branch), "main ↑1");
        assert_eq!(branch_label("No commits yet on main"), "main · sin commits");
        assert_eq!(branch_label("dev...origin/dev [ahead 2, behind 3]"), "dev ↑2 ↓3");
        assert_eq!(branch_label("main"), "main");
    }

    /// Flujo completo sobre un repo temporal: ver cambio → stage → commit → descartar (con confirmación).
    #[test]
    fn stage_commit_discard() {
        let dir = std::env::temp_dir().join(format!("kp-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "kp@test"], &["config", "user.name", "kp"]] {
            git(&dir, args).unwrap();
        }
        std::fs::write(dir.join("a.txt"), "hola\n").unwrap();
        let (tx, _rx) = mpsc::channel();
        let mut v = GitView::open(&dir, None, tx).unwrap();
        assert_eq!(v.files.len(), 1);
        assert!(v.diff.iter().any(|l| l == "+hola"));

        v.press(Btn::Stage);
        assert_eq!(v.files[0].x, 'A', "{}", v.msg);
        v.press(Btn::Commit);
        for c in "primer commit".chars() {
            v.key(KeyEvent::from(K::Char(c)));
        }
        v.key(KeyEvent::from(K::Enter));
        assert!(v.files.is_empty(), "{}", v.msg);
        assert!(git(&dir, &["log", "--oneline"]).unwrap().contains("primer commit"));
        assert_eq!(branch(&dir).as_deref(), Some("main"));
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(branch(&sub).as_deref(), Some("main"), "desde una subcarpeta");

        std::fs::write(dir.join("a.txt"), "cambiado\n").unwrap();
        v.refresh();
        assert!(v.diff.iter().any(|l| l == "-hola") && v.diff.iter().any(|l| l == "+cambiado"));
        v.press(Btn::Discard); // primera vez solo arma
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "cambiado\n");
        v.press(Btn::Discard);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap().trim(), "hola");
        assert!(v.files.is_empty());

        // Push no publica a la primera: solo pide confirmación.
        v.press(Btn::Push);
        assert!(v.msg.contains("otra vez") && !v.busy, "{}", v.msg);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
