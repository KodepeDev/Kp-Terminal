mod config;
mod ctl;
mod git;
mod keys;
mod session;
mod ui;

use config::{Config, THEMES, Theme};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event as CtEvent,
    KeyCode as K, KeyEvent, KeyEventKind, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use session::{Session, Status};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

pub enum Event {
    Output(usize, Vec<u8>),
    Exit(usize),
    Input(CtEvent),
    /// args, cuerpo, canal de respuesta
    Ctl(Vec<String>, String, Sender<String>),
    /// Resultado de git pull/push en segundo plano.
    GitDone(String),
}

#[derive(PartialEq, Clone, Copy)]
pub enum Mode {
    Normal,
    Prefix,
    Palette,
    Broadcast,
}

/// Aviso flotante (esquina superior derecha) cuando un agente termina o se bloquea.
pub struct Toast {
    pub text: String,
    pub status: Status,
    pub session: usize,
    pub at: Instant,
}

/// Entrada de la paleta.
pub struct Item {
    pub icon: &'static str,
    pub label: String,
    pub action: Action,
    /// Posiciones (en caracteres) que coinciden con la búsqueda, para resaltarlas.
    pub hits: Vec<usize>,
}

#[derive(Clone)]
pub enum Action {
    Spawn(usize, bool),
    Focus(usize),
    Grid,
    Broadcast,
    Close,
    Quit,
    Git,
    Theme(usize),
}

pub struct App {
    pub sessions: Vec<Session>,
    pub focus: usize,
    pub mode: Mode,
    pub grid: bool,
    pub input: String,
    pub sel: usize,
    pub msg: String,
    pub project: PathBuf,
    pub cfg: Config,
    pane: (u16, u16),
    env: Vec<(&'static str, String)>,
    tx: Sender<Event>,
    next_id: usize,
    confirm: Option<char>,
    quit: bool,
    pub theme: Theme,
    /// Panel de revisión git abierto sobre la carpeta de una sesión.
    pub git: Option<git::GitView>,
    /// Barra lateral visible (Ctrl+Space s).
    pub sidebar: bool,
    pub toasts: Vec<Toast>,
    /// Origen del reloj de animación (spinner).
    pub started: Instant,
    /// Zonas clicables calculadas en el último render.
    pub hit_new: ratatui::layout::Rect,
    pub hit_toasts: Vec<(ratatui::layout::Rect, usize)>,
    last_clock: u64,
}

/// Coincidencia difusa: posiciones de `s` donde aparecen, en orden, las letras de `q`.
pub fn fuzzy_pos(q: &str, s: &str) -> Option<Vec<usize>> {
    let mut it = s.chars().enumerate();
    let mut hits = vec![];
    for c in q.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase) {
        hits.push(it.by_ref().find(|(_, x)| x.to_lowercase().eq(std::iter::once(c)))?.0);
    }
    Some(hits)
}

pub fn fuzzy(q: &str, s: &str) -> bool {
    fuzzy_pos(q, s).is_some()
}

impl App {
    fn new(cfg: Config, project: PathBuf, env: Vec<(&'static str, String)>, tx: Sender<Event>) -> Self {
        App {
            sessions: vec![],
            focus: 0,
            mode: Mode::Normal,
            grid: false,
            input: String::new(),
            sel: 0,
            msg: String::new(),
            project,
            cfg,
            pane: (24, 80),
            env,
            tx,
            next_id: 1,
            confirm: None,
            quit: false,
            theme: Theme::default(),
            git: None,
            sidebar: true,
            toasts: vec![],
            started: Instant::now(),
            hit_new: Default::default(),
            hit_toasts: vec![],
            last_clock: 0,
        }
    }

    pub fn sidebar_w(&self) -> u16 {
        if self.sidebar { ui::SIDEBAR } else { 0 }
    }

    pub fn cur(&mut self) -> Option<&mut Session> {
        self.sessions.get_mut(self.focus)
    }

    fn idx(&self, id: usize) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == id)
    }

    fn any_working(&self) -> bool {
        self.sessions.iter().any(|s| s.agent && s.status == Status::Working)
    }

    pub fn set_focus(&mut self, i: usize) {
        if i < self.sessions.len() {
            let changed = i != self.focus;
            self.focus = i;
            self.sessions[i].attention = false;
            if changed && self.git.is_some() {
                self.open_git(); // el panel sigue a la sesión enfocada
            }
        }
    }

    /// Redimensiona todas las sesiones al área principal (la llama ui::draw).
    pub fn resize(&mut self, rows: u16, cols: u16) {
        if self.pane != (rows, cols) {
            self.pane = (rows, cols);
            self.sessions.iter_mut().for_each(|s| s.resize(rows, cols));
        }
    }

    fn shell_preset(&self) -> usize {
        self.cfg.presets.iter().position(|p| !p.agent).unwrap_or(0)
    }

    pub fn spawn(&mut self, preset: usize, worktree: bool, dir: Option<PathBuf>, focus: bool) -> Result<usize, String> {
        let p = self.cfg.presets.get(preset).ok_or("preset inexistente")?.clone();
        let mut cwd = dir.unwrap_or_else(|| self.project.clone());
        let mut name = p.name.clone();
        if worktree {
            cwd = git::worktree(&cwd, &p.name)?;
            name.push_str(" ⎇");
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut env = self.env.clone();
        env.push(("KP_SESSION", id.to_string()));
        let s = Session::spawn(id, name, p.agent, &p.cmd, &cwd, self.pane, self.cfg.scrollback, &env, self.tx.clone())?;
        self.sessions.push(s);
        if focus || self.sessions.len() == 1 {
            self.set_focus(self.sessions.len() - 1);
        }
        Ok(id)
    }

    fn close(&mut self, i: usize) {
        if i < self.sessions.len() {
            let mut s = self.sessions.remove(i);
            s.kill();
            if self.focus >= self.sessions.len() {
                self.focus = self.sessions.len().saturating_sub(1);
            }
        }
    }

    /// Pide repetir la tecla si la acción mataría agentes trabajando.
    fn confirmed(&mut self, key: char, risky: bool, pending: Option<char>) -> bool {
        if !risky || pending == Some(key) {
            return true;
        }
        self.confirm = Some(key);
        self.msg = format!("Hay agentes trabajando: repite Ctrl+Space {key} para confirmar");
        false
    }

    /// Abre el panel git sobre la carpeta de la sesión enfocada (su worktree, si es un agente aislado).
    fn open_git(&mut self) {
        let (dir, id) = match self.sessions.get(self.focus) {
            Some(s) => (s.cwd.clone(), Some(s.id).filter(|_| s.agent)),
            None => (self.project.clone(), None),
        };
        match git::GitView::open(&dir, id, self.tx.clone()) {
            Ok(g) => {
                self.git = Some(g);
                self.grid = false;
            }
            Err(e) => {
                self.git = None;
                self.msg = e;
            }
        }
    }

    fn git_req(&mut self, req: Option<git::Req>) {
        match req {
            Some(git::Req::Close) => self.git = None,
            Some(git::Req::Ask(prompt)) => {
                let id = self.git.as_ref().and_then(|g| g.session);
                if let Some(s) = id.and_then(|id| self.idx(id)).map(|i| &mut self.sessions[i]) {
                    s.send_prompt(&prompt);
                }
            }
            None => {}
        }
    }

    pub fn palette_items(&self) -> Vec<Item> {
        let mut v: Vec<(&'static str, String, Action)> = vec![];
        for (i, p) in self.cfg.presets.iter().enumerate() {
            let icon = if p.agent { "▶" } else { "$" };
            v.push((icon, format!("nuevo {}  ·  {}", p.name, p.cmd), Action::Spawn(i, false)));
        }
        for (i, p) in self.cfg.presets.iter().enumerate().filter(|(_, p)| p.agent) {
            v.push(("⎇", format!("nuevo {} en git worktree aislado", p.name), Action::Spawn(i, true)));
        }
        for (i, s) in self.sessions.iter().enumerate() {
            let what = if s.agent { s.title().to_string() } else { s.cwd.display().to_string() };
            v.push(("→", format!("ir a {} {}  ·  {}  {what}", i + 1, s.name, s.status.label()), Action::Focus(i)));
        }
        v.push(("±", "revisar cambios (git): diff, stage, commit, push".into(), Action::Git));
        v.push(("▦", "vista grid (overview de agentes)".into(), Action::Grid));
        v.push(("≋", "broadcast: mismo prompt a todos los agentes".into(), Action::Broadcast));
        v.push(("✕", "cerrar sesión actual".into(), Action::Close));
        v.push(("⏻", "salir de kp".into(), Action::Quit));
        for (i, (name, ..)) in THEMES.iter().enumerate() {
            v.push(("◐", format!("tema: {name}"), Action::Theme(i)));
        }
        let mut items: Vec<Item> = v
            .into_iter()
            .filter_map(|(icon, label, action)| fuzzy_pos(&self.input, &label).map(|hits| Item { icon, label, action, hits }))
            .collect();
        // Relevancia: primero el texto literal, luego las coincidencias más compactas (orden estable).
        let q: String = self.input.to_lowercase().split_whitespace().collect();
        items.sort_by_key(|it| {
            let literal = it.label.to_lowercase().replace(' ', "").contains(&q);
            let spread = it.hits.last().zip(it.hits.first()).map_or(0, |(l, f)| l - f);
            (!literal, spread)
        });
        items
    }

    /// Tema que se previsualiza al recorrer la paleta (sin aplicarlo todavía).
    pub fn preview_theme(&self) -> Option<Theme> {
        if self.mode != Mode::Palette {
            return None;
        }
        let items = self.palette_items();
        match items.get(self.sel.min(items.len().saturating_sub(1)))?.action {
            Action::Theme(i) => self.cfg.theme_named(THEMES[i].0).ok(),
            _ => None,
        }
    }

    fn run(&mut self, a: Action) {
        let pending = self.confirm.take();
        match a {
            Action::Spawn(p, wt) => {
                if let Err(e) = self.spawn(p, wt, None, true) {
                    self.msg = e;
                }
                self.grid = false;
            }
            Action::Focus(i) => {
                self.set_focus(i);
                self.grid = false;
            }
            Action::Grid => self.grid = !self.grid,
            Action::Git => self.open_git(),
            Action::Theme(i) => match self.cfg.theme_named(THEMES[i].0) {
                Ok(t) => {
                    self.theme = t;
                    self.msg = format!("tema {} (fíjalo con theme = \"{}\" en la config)", THEMES[i].0, THEMES[i].0);
                }
                Err(e) => self.msg = e,
            },
            Action::Broadcast => {
                self.input.clear();
                self.mode = Mode::Broadcast;
            }
            Action::Close => {
                let risky = self.sessions.get(self.focus).is_some_and(|s| s.agent && s.status == Status::Working);
                if self.confirmed('w', risky, pending) {
                    self.close(self.focus);
                }
            }
            Action::Quit => {
                if self.confirmed('q', self.any_working(), pending) {
                    self.quit = true;
                }
            }
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Release {
            return;
        }
        if self.mode != Mode::Prefix {
            self.msg.clear();
        }
        match self.mode {
            Mode::Prefix => {
                self.mode = Mode::Normal;
                self.prefix_key(k);
            }
            Mode::Palette | Mode::Broadcast => self.line_key(k),
            Mode::Normal if keys::is_prefix(&k) => self.mode = Mode::Prefix,
            Mode::Normal if self.git.is_some() => {
                let req = self.git.as_mut().and_then(|g| g.key(k));
                self.git_req(req);
            }
            Mode::Normal if self.grid => self.grid_key(k),
            Mode::Normal => {
                if let Some(s) = self.cur() {
                    if s.scroll > 0 {
                        s.scroll_by(-(s.scroll as i32));
                    }
                    let bytes = keys::encode(&k, s.parser.screen().application_cursor());
                    s.write(&bytes);
                }
            }
        }
    }

    fn prefix_key(&mut self, k: KeyEvent) {
        let half = (self.pane.0 / 2).max(1) as i32;
        match k.code {
            _ if keys::is_prefix(&k) => {
                if let Some(s) = self.cur() {
                    s.write(&[0]);
                }
            }
            K::Char('n') => self.run(Action::Spawn(self.shell_preset(), false)),
            K::Char('p' | 'a') => {
                self.input.clear();
                self.sel = 0;
                self.mode = Mode::Palette;
            }
            K::Char('g') => self.run(Action::Grid),
            K::Char('r') => self.run(Action::Git),
            K::Char('s') => self.sidebar = !self.sidebar,
            K::Char('b') => self.run(Action::Broadcast),
            K::Char('w') => self.run(Action::Close),
            K::Char('q') => self.run(Action::Quit),
            K::Char('j') | K::Down | K::Tab if !self.sessions.is_empty() => {
                self.set_focus((self.focus + 1) % self.sessions.len())
            }
            K::Char('k') | K::Up if !self.sessions.is_empty() => {
                self.set_focus((self.focus + self.sessions.len() - 1) % self.sessions.len())
            }
            K::Char(c @ '1'..='9') => self.set_focus(c as usize - '1' as usize),
            K::Char('u') | K::PageUp => {
                if let Some(s) = self.cur() {
                    s.scroll_by(half)
                }
            }
            K::Char('d') | K::PageDown => {
                if let Some(s) = self.cur() {
                    s.scroll_by(-half)
                }
            }
            _ => {}
        }
    }

    fn grid_key(&mut self, k: KeyEvent) {
        let n = self.sessions.len();
        if n == 0 {
            self.grid = false;
            return;
        }
        let cols = ui::grid_cols(n);
        let f = self.focus;
        match k.code {
            K::Left | K::Char('h') => self.set_focus((f + n - 1) % n),
            K::Right | K::Char('l') | K::Tab => self.set_focus((f + 1) % n),
            K::Up | K::Char('k') => self.set_focus(f.saturating_sub(cols)),
            K::Down | K::Char('j') => self.set_focus((f + cols).min(n - 1)),
            K::Char(c @ '1'..='9') => self.set_focus(c as usize - '1' as usize),
            K::Enter | K::Esc => self.grid = false,
            _ => {}
        }
    }

    fn line_key(&mut self, k: KeyEvent) {
        match k.code {
            K::Esc => self.mode = Mode::Normal,
            K::Backspace => {
                self.input.pop();
                self.sel = 0;
            }
            K::Up => self.sel = self.sel.saturating_sub(1),
            K::Down | K::Tab => self.sel = (self.sel + 1).min(self.palette_items().len().saturating_sub(1)),
            K::Enter if self.mode == Mode::Palette => {
                self.mode = Mode::Normal;
                let items = self.palette_items();
                if let Some(item) = items.get(self.sel.min(items.len().saturating_sub(1))) {
                    self.run(item.action.clone());
                }
            }
            K::Enter => {
                self.mode = Mode::Normal;
                let text = std::mem::take(&mut self.input);
                let mut n = 0;
                for s in self.sessions.iter_mut().filter(|s| s.agent && s.status != Status::Exited) {
                    s.send_prompt(&text);
                    n += 1;
                }
                self.msg = format!("prompt enviado a {n} agentes");
            }
            K::Char(c) if !keys::is_prefix(&k) => {
                self.input.push(c);
                self.sel = 0;
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, m: event::MouseEvent) {
        let in_sidebar = m.column < self.sidebar_w();
        let hit = |r: &ratatui::layout::Rect| r.contains(ratatui::layout::Position::new(m.column, m.row));
        if m.kind == MouseEventKind::Down(MouseButton::Left) {
            if let Some(&(_, id)) = self.hit_toasts.iter().find(|(r, _)| hit(r)) {
                self.toasts.retain(|t| t.session != id);
                if let Some(i) = self.idx(id) {
                    self.git = None;
                    self.grid = false;
                    self.set_focus(i);
                }
                return;
            }
            if hit(&self.hit_new) {
                self.input.clear();
                self.sel = 0;
                self.mode = Mode::Palette;
                return;
            }
        }
        if let (Some(g), false) = (self.git.as_mut(), in_sidebar) {
            let req = match m.kind {
                MouseEventKind::Down(MouseButton::Left) => g.click(m.column, m.row),
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    g.wheel(m.column, m.row, m.kind == MouseEventKind::ScrollUp);
                    None
                }
                _ => None,
            };
            return self.git_req(req);
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if in_sidebar && m.row >= 1 => {
                self.set_focus((m.row as usize - 1) / 2);
                self.grid = false;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if !in_sidebar && !self.grid => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if let Some(s) = self.cur() {
                    let scr = s.parser.screen();
                    if scr.alternate_screen() {
                        // Apps a pantalla completa (vim, less...): la rueda son flechas.
                        let arrow = if scr.application_cursor() { "O" } else { "[" };
                        let seq = format!("\x1b{arrow}{}", if up { 'A' } else { 'B' }).repeat(3);
                        s.write(seq.as_bytes());
                    } else {
                        s.scroll_by(if up { 3 } else { -3 });
                    }
                }
            }
            _ => {}
        }
    }

    fn on_ctl(&mut self, args: Vec<String>, body: String) -> String {
        let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
        let target = arg(1).parse().ok().and_then(|id| self.idx(id));
        match (arg(0), target) {
            ("list", _) => self
                .sessions
                .iter()
                .map(|s| format!("{}\t{}\t{}\t{}", s.id, s.name, s.status.label(), s.cwd.display()))
                .collect::<Vec<_>>()
                .join("\n"),
            ("spawn", _) => {
                let Some(p) = self.cfg.presets.iter().position(|p| p.name == arg(1)) else {
                    let names: Vec<_> = self.cfg.presets.iter().map(|p| p.name.as_str()).collect();
                    return format!("err: preset '{}' no existe ({})", arg(1), names.join(", "));
                };
                let wt = args.iter().any(|a| a == "--worktree");
                let dir = args.iter().skip(2).find(|a| *a != "--worktree").map(|d| self.project.join(d));
                match self.spawn(p, wt, dir, false) {
                    Ok(id) => id.to_string(),
                    Err(e) => format!("err: {e}"),
                }
            }
            ("send" | "read" | "status" | "focus" | "close", None) => format!("err: sesión '{}' no existe", arg(1)),
            ("send", Some(i)) => {
                self.sessions[i].send_prompt(&body);
                "ok".into()
            }
            ("read", Some(i)) => self.sessions[i].tail(arg(2).parse().unwrap_or(50)),
            ("status", Some(i)) => self.sessions[i].status.label().into(),
            ("focus", Some(i)) => {
                self.set_focus(i);
                "ok".into()
            }
            ("close", Some(i)) => {
                self.close(i);
                "ok".into()
            }
            _ => "err: comando desconocido (kp ctl help)".into(),
        }
    }

    fn handle(&mut self, ev: Event) {
        match ev {
            Event::Output(id, bytes) => {
                if let Some(i) = self.idx(id) {
                    self.sessions[i].feed(&bytes);
                }
            }
            Event::Exit(id) => {
                if let Some(i) = self.idx(id) {
                    // Los shells se cierran solos (como tmux); los agentes quedan visibles con su salida final.
                    if self.sessions[i].agent {
                        self.sessions[i].mark_exited();
                    } else {
                        self.close(i);
                    }
                }
            }
            Event::Input(CtEvent::Key(k)) => self.on_key(k),
            Event::Input(CtEvent::Mouse(m)) => self.on_mouse(m),
            Event::Input(CtEvent::Paste(text)) => match self.mode {
                Mode::Palette | Mode::Broadcast => self.input.push_str(&text),
                _ if self.git.is_some() => self.git.as_mut().unwrap().paste(&text),
                _ => {
                    if let Some(s) = self.cur() {
                        s.paste(&text);
                    }
                }
            },
            Event::Input(_) => {}
            Event::GitDone(msg) => match self.git.as_mut() {
                Some(g) => g.done(msg),
                None => self.msg = msg,
            },
            Event::Ctl(args, body, reply) => {
                let r = self.on_ctl(args, body);
                let _ = reply.send(r);
            }
        }
    }

    /// Recalcula estados; devuelve si hay que repintar.
    fn tick(&mut self) -> bool {
        let (mut changed, mut ring) = (false, false);
        for (i, s) in self.sessions.iter_mut().enumerate() {
            let (c, r) = s.tick(&self.cfg.waiting_patterns, i == self.focus && !self.grid && self.git.is_none());
            changed |= c;
            if r {
                ring = true;
                let text = match s.status {
                    Status::Waiting => format!("{} necesita tu decisión", s.name),
                    Status::Exited => format!("{} salió", s.name),
                    _ => format!("{} terminó", s.name),
                };
                self.toasts.retain(|t| t.session != s.id);
                self.toasts.push(Toast { text, status: s.status, session: s.id, at: Instant::now() });
            }
        }
        if ring && self.cfg.bell {
            let _ = std::io::stdout().write_all(b"\x07");
        }
        let n = self.toasts.len();
        self.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(6));
        self.toasts.truncate(4);
        // Repinta para el spinner (si algo trabaja), la caducidad de avisos y los relojes (1 vez/s).
        let clock = self.started.elapsed().as_secs();
        let tick_clock = clock != self.last_clock;
        self.last_clock = clock;
        let animating = self.sessions.iter().any(|s| s.status == Status::Working);
        changed || animating || tick_clock || n != self.toasts.len()
    }
}

const HELP: &str = "kp — terminal para orquestar agentes de desarrollo

uso:
  kp [carpeta]      abre kp con la carpeta como proyecto (por defecto, la actual)
  kp ctl <cmd>      controla kp desde dentro de una sesión (kp ctl help)

teclas (prefijo Ctrl+Space):
  n shell nuevo · p/a paleta · r revisar (git) · g grid · b broadcast · s panel lateral · j/k o 1-9 cambiar sesión
  u/d scroll · w cerrar · q salir · Ctrl+Space dos veces envía Ctrl+Space";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ctl") => std::process::exit(ctl::client(&args[1..])),
        Some("-h" | "--help" | "help") => return println!("{HELP}\n\nconfig: {}", config::path().display()),
        Some("-V" | "--version") => return println!("kp {}", env!("CARGO_PKG_VERSION")),
        _ => {}
    }
    let project = args
        .first()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .and_then(|p| std::path::absolute(p).ok())
        .unwrap_or_default();
    if !project.is_dir() {
        eprintln!("kp: '{}' no es una carpeta", project.display());
        std::process::exit(2);
    }
    if let Err(e) = run(project) {
        eprintln!("kp: {e}");
        std::process::exit(1);
    }
}

fn run(project: PathBuf) -> std::io::Result<()> {
    let (cfg, cfg_err) = config::load();
    let (tx, rx) = mpsc::channel();
    let (addr, token) = ctl::serve(tx.clone())?;
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let env = vec![("KP_ADDR", addr), ("KP_TOKEN", token), ("KP_BIN", exe)];

    let input_tx = tx.clone();
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if input_tx.send(Event::Input(ev)).is_err() {
                break;
            }
        }
    });

    let mut term = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let mut app = App::new(cfg, project, env, tx);
    app.resize(term.size()?.height.saturating_sub(2), term.size()?.width.saturating_sub(ui::SIDEBAR));
    if let Err(e) = app.spawn(app.shell_preset(), false, None, true) {
        app.msg = e;
    }
    match app.cfg.theme_named(&app.cfg.theme) {
        Ok(t) => app.theme = t,
        Err(e) => app.msg = e,
    }
    if let Some(e) = cfg_err {
        app.msg = e;
    }

    let result = (|| -> std::io::Result<()> {
        let mut dirty = true;
        let mut last_tick = Instant::now();
        loop {
            if dirty {
                term.draw(|f| ui::draw(f, &mut app))?;
            }
            dirty = match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(ev) => {
                    app.handle(ev);
                    // Agrupa ráfagas de salida en un solo frame (~120 fps máx.).
                    let until = Instant::now() + Duration::from_millis(8);
                    while Instant::now() < until {
                        match rx.try_recv() {
                            Ok(ev) => app.handle(ev),
                            Err(_) => break,
                        }
                    }
                    true
                }
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            // La detección de estado lee la pantalla de cada sesión: 5 veces por segundo basta.
            if last_tick.elapsed() >= Duration::from_millis(200) {
                last_tick = Instant::now();
                dirty |= app.tick();
            }
            if app.quit {
                break;
            }
        }
        Ok(())
    })();

    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    app.sessions.iter_mut().for_each(Session::kill);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matches() {
        assert_eq!(fuzzy_pos("ncl", "nuevo claude"), Some(vec![0, 6, 7]));
        assert_eq!(fuzzy_pos("ñ", "año"), Some(vec![1]));
        assert!(fuzzy("ncl", "nuevo claude"));
        assert!(fuzzy("", "x"));
        assert!(fuzzy("GRID", "vista grid"));
        assert!(!fuzzy("zz", "nuevo claude"));
    }

    #[test]
    fn palette_ranks_literal_first() {
        let (tx, _rx) = mpsc::channel();
        let mut app = App::new(Config::default(), PathBuf::from("C:/Temp/x"), vec![], tx);
        app.input = "tema".into();
        assert!(app.palette_items()[0].label.starts_with("tema:"));
        app.input = "grid".into();
        assert!(matches!(app.palette_items()[0].action, Action::Grid));
    }

    /// API ctl de punta a punta: servidor TCP + token + despacho a App + sesión PTY real.
    #[test]
    fn ctl_roundtrip() {
        let (tx, rx) = mpsc::channel();
        let (addr, tok) = ctl::serve(tx.clone()).unwrap();
        let mut cfg = Config::default();
        cfg.presets[0].cmd = if cfg!(windows) { "cmd.exe /Q /K".into() } else { "sh".into() };
        let mut app = App::new(cfg, std::env::temp_dir(), vec![], tx);
        let (a2, t2) = (addr.clone(), tok.clone());
        let client = std::thread::spawn(move || {
            let call = |a: &[&str], body: &str| {
                let a: Vec<String> = a.iter().map(|s| s.to_string()).collect();
                ctl::request(&a2, &t2, &a, body).unwrap()
            };
            let id = call(&["spawn", "shell"], "");
            assert_eq!(call(&["send", &id], "echo kp-ok-123"), "ok");
            let mut seen = String::new();
            for _ in 0..60 {
                seen = call(&["read", &id], "");
                if seen.matches("kp-ok-123").count() >= 2 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            assert!(seen.matches("kp-ok-123").count() >= 2, "pantalla: {seen}");
            assert!(call(&["list"], "").contains("shell"));
            assert!(call(&["status", "999"], "").starts_with("err"));
            assert!(ctl::request(&a2, "malo", &["list".into()], "").unwrap().contains("token"));
        });
        while !client.is_finished() {
            if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
                app.handle(ev);
            }
        }
        app.sessions.iter_mut().for_each(Session::kill);
        client.join().unwrap();
    }
}
