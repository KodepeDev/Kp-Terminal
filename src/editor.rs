//! Editor de comandos fijo abajo (estilo Warp): edición multilínea, historial con filtro por prefijo,
//! sugerencia fantasma desde el historial y autocompletado de rutas con Tab.
use ratatui::crossterm::event::{KeyCode as K, KeyEvent, KeyModifiers as M};
use std::path::{Path, PathBuf};

const HISTORY_MAX: usize = 2000;

/// Historial global de comandos, persistido en un archivo (una línea por comando).
pub struct History {
    pub items: Vec<String>,
    path: Option<PathBuf>,
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match (c, c == '\\') {
            (_, true) => match it.next() {
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            },
            (c, false) => out.push(c),
        }
    }
    out
}

impl History {
    pub fn load(path: Option<PathBuf>) -> History {
        let items = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| s.lines().map(unescape).collect::<Vec<_>>())
            .unwrap_or_default();
        // Sin duplicados: se conserva la aparición más reciente de cada comando.
        let mut seen = std::collections::HashSet::new();
        let mut items: Vec<String> = items.into_iter().rev().filter(|i| seen.insert(i.clone())).take(HISTORY_MAX).collect();
        items.reverse();
        History { items, path }
    }

    pub fn push(&mut self, cmd: &str) {
        let cmd = cmd.trim_end();
        if cmd.is_empty() {
            return;
        }
        // Un comando repetido sube al final (más reciente) en vez de duplicarse.
        self.items.retain(|i| i != cmd);
        self.items.push(cmd.to_string());
        if let Some(p) = &self.path {
            use std::io::Write;
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            // ponytail: el archivo crece sin límite; se recorta a HISTORY_MAX solo al cargar.
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = writeln!(f, "{}", escape(cmd));
            }
        }
    }
}

/// Lo que el editor pide a la app tras una tecla.
#[derive(Debug, PartialEq)]
pub enum Out {
    None,
    /// Ejecutar este comando en el shell.
    Submit(String),
    /// La tecla no es del editor: mandarla tal cual al shell.
    Pass,
    /// Entrar en el modo de selección de bloques.
    Blocks,
    /// Desplazar el historial de la terminal.
    Scroll(i32),
}

#[derive(Default)]
pub struct Editor {
    pub buf: String,
    /// Posición del cursor en bytes (siempre en un límite de carácter).
    pub cur: usize,
    hist_pos: Option<usize>,
    draft: String,
    /// Opciones de autocompletado cuando hay varias.
    pub candidates: Vec<String>,
}

impl Editor {
    fn prev(&self, i: usize) -> usize {
        self.buf[..i].char_indices().next_back().map_or(0, |(j, _)| j)
    }

    fn next(&self, i: usize) -> usize {
        self.buf[i..].chars().next().map_or(i, |c| i + c.len_utf8())
    }

    fn line_start(&self) -> usize {
        self.buf[..self.cur].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.buf[self.cur..].find('\n').map_or(self.buf.len(), |i| self.cur + i)
    }

    fn word_left(&self) -> usize {
        let s = &self.buf[..self.cur];
        let trimmed = s.trim_end();
        trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1)
    }

    fn word_right(&self) -> usize {
        let s = &self.buf[self.cur..];
        let start = s.len() - s.trim_start().len();
        s[start..].find(char::is_whitespace).map_or(self.buf.len(), |i| self.cur + start + i)
    }

    fn edited(&mut self) {
        self.hist_pos = None;
        self.candidates.clear();
    }

    pub fn insert(&mut self, s: &str) {
        self.buf.insert_str(self.cur, s);
        self.cur += s.len();
        self.edited();
    }

    fn delete_range(&mut self, a: usize, b: usize) {
        self.buf.replace_range(a..b, "");
        self.cur = a;
        self.edited();
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.cur = 0;
        self.edited();
    }

    pub fn set(&mut self, s: &str) {
        self.buf = s.to_string();
        self.cur = self.buf.len();
        self.candidates.clear();
    }

    pub fn take(&mut self) -> String {
        let s = std::mem::take(&mut self.buf);
        self.cur = 0;
        self.edited();
        s
    }

    /// Resto sugerido desde el historial (texto fantasma), si el cursor está al final.
    pub fn suggestion<'a>(&self, hist: &'a History) -> Option<&'a str> {
        if self.buf.is_empty() || self.cur != self.buf.len() || self.buf.contains('\n') {
            return None;
        }
        hist.items.iter().rev().find(|h| h.len() > self.buf.len() && h.starts_with(&self.buf)).map(|h| &h[self.buf.len()..])
    }

    /// Navega el historial filtrando por lo escrito antes de empezar (como fish/Warp).
    fn history(&mut self, hist: &History, back: bool) {
        if self.hist_pos.is_none() {
            self.draft = self.buf.clone();
        }
        let matches = |h: &String| h.starts_with(&self.draft) && *h != self.buf;
        let found = if back {
            let end = self.hist_pos.unwrap_or(hist.items.len());
            hist.items[..end].iter().rposition(matches)
        } else {
            let start = self.hist_pos.map_or(hist.items.len(), |p| p + 1);
            hist.items.get(start..).and_then(|rest| rest.iter().position(matches)).map(|i| start + i)
        };
        match found {
            Some(i) => {
                self.buf = hist.items[i].clone();
                self.hist_pos = Some(i);
            }
            None if !back => {
                self.buf = self.draft.clone();
                self.hist_pos = None;
            }
            None => return,
        }
        self.cur = self.buf.len();
        self.candidates.clear();
    }

    /// Autocompleta la última palabra como ruta relativa a `cwd`.
    pub fn complete(&mut self, cwd: &Path) {
        self.cur = self.buf.len();
        let start = self.buf.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        let token = self.buf[start..].to_string();
        let split = token.rfind(['/', '\\']).map_or(0, |i| i + 1);
        let (dir_part, prefix) = token.split_at(split);
        let sep = dir_part.chars().last().unwrap_or(std::path::MAIN_SEPARATOR);
        let base = if dir_part.is_empty() { cwd.to_path_buf() } else { cwd.join(dir_part) };
        let Ok(entries) = std::fs::read_dir(&base) else { return };
        let lower = prefix.to_lowercase();
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let mut n = e.file_name().to_string_lossy().to_string();
                if !n.to_lowercase().starts_with(&lower) {
                    return None;
                }
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    n.push(sep);
                }
                Some(n)
            })
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        let completion = match names.as_slice() {
            [] => return,
            [one] => one.clone(),
            many => {
                // Prefijo común (sin distinguir mayúsculas), tomado del primero.
                let first = &many[0];
                let len = first
                    .char_indices()
                    .map(|(i, c)| i + c.len_utf8())
                    .take_while(|&end| many.iter().all(|n| n.to_lowercase().starts_with(&first[..end].to_lowercase())))
                    .last()
                    .unwrap_or(0);
                self.candidates = many.iter().take(50).cloned().collect();
                first[..len].to_string()
            }
        };
        if completion.len() >= prefix.len() {
            // ponytail: no añade comillas a nombres con espacios.
            let cands = std::mem::take(&mut self.candidates);
            self.buf.truncate(start + split);
            self.buf.push_str(&completion);
            self.cur = self.buf.len();
            self.hist_pos = None;
            self.candidates = cands;
        }
    }

    pub fn key(&mut self, k: KeyEvent, hist: &History, cwd: &Path) -> Out {
        let m = k.modifiers;
        let altgr = m.contains(M::CONTROL | M::ALT);
        let ctrl = m.contains(M::CONTROL) && !altgr;
        let alt = m.contains(M::ALT) && !altgr;
        match k.code {
            K::Enter if m.contains(M::SHIFT) || alt => self.insert("\n"),
            K::Enter => return Out::Submit(self.take()),
            K::Char(c) if ctrl => match c {
                'c' if !self.buf.is_empty() => self.clear(),
                'u' => self.delete_range(self.line_start(), self.cur),
                'k' => self.delete_range(self.cur, self.line_end()),
                'w' => self.delete_range(self.word_left(), self.cur),
                'a' => self.cur = self.line_start(),
                'e' => self.cur = self.line_end(),
                _ => return Out::Pass,
            },
            K::Char(c) => self.insert(c.encode_utf8(&mut [0; 4])),
            K::Backspace if self.cur > 0 => {
                let a = if ctrl || alt { self.word_left() } else { self.prev(self.cur) };
                self.delete_range(a, self.cur);
            }
            K::Delete if self.cur < self.buf.len() => {
                let b = if ctrl { self.word_right() } else { self.next(self.cur) };
                self.buf.replace_range(self.cur..b, "");
                self.edited();
            }
            K::Left => self.cur = if ctrl || alt { self.word_left() } else { self.prev(self.cur) },
            K::Right | K::End if self.cur == self.buf.len() => {
                if let Some(rest) = self.suggestion(hist).map(String::from) {
                    self.insert(&rest);
                }
            }
            K::Right => self.cur = if ctrl || alt { self.word_right() } else { self.next(self.cur) },
            K::Home => self.cur = self.line_start(),
            K::End => self.cur = self.line_end(),
            K::Up if ctrl => return Out::Blocks,
            K::Up if self.line_start() == 0 => self.history(hist, true),
            K::Down if self.line_end() == self.buf.len() && self.hist_pos.is_some() => self.history(hist, false),
            K::Up | K::Down => {
                // Mover entre líneas del comando conservando la columna.
                let col = self.buf[self.line_start()..self.cur].chars().count();
                let target = if k.code == K::Up {
                    let prev_end = self.line_start() - 1;
                    self.buf[..prev_end].rfind('\n').map_or(0, |i| i + 1)
                } else {
                    match self.buf[self.cur..].find('\n') {
                        Some(i) => self.cur + i + 1,
                        None => return Out::None,
                    }
                };
                let line_len = self.buf[target..].find('\n').unwrap_or(self.buf.len() - target);
                let off: usize = self.buf[target..target + line_len].chars().take(col).map(char::len_utf8).sum();
                self.cur = target + off;
            }
            K::Tab => self.complete(cwd),
            K::Esc if !self.candidates.is_empty() => self.candidates.clear(),
            K::Esc => self.clear(),
            K::PageUp => return Out::Scroll(10),
            K::PageDown => return Out::Scroll(-10),
            _ if self.buf.is_empty() => return Out::Pass,
            _ => {}
        }
        Out::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: K) -> KeyEvent {
        KeyEvent::new(code, M::NONE)
    }

    fn typed(e: &mut Editor, s: &str, h: &History) {
        for c in s.chars() {
            e.key(key(K::Char(c)), h, Path::new("."));
        }
    }

    #[test]
    fn edits_and_submits() {
        let h = History::load(None);
        let mut e = Editor::default();
        typed(&mut e, "git stats", &h);
        e.key(key(K::Left), &h, Path::new("."));
        e.key(key(K::Char('u')), &h, Path::new("."));
        assert_eq!(e.buf, "git status");
        e.key(KeyEvent::new(K::Backspace, M::CONTROL), &h, Path::new("."));
        assert_eq!(e.buf, "git s");
        e.key(key(K::End), &h, Path::new("."));
        e.key(KeyEvent::new(K::Enter, M::SHIFT), &h, Path::new("."));
        typed(&mut e, "ñu", &h);
        assert_eq!(e.buf, "git s\nñu");
        e.key(key(K::Up), &h, Path::new("."));
        assert_eq!(&e.buf[..e.cur], "gi");
        assert_eq!(e.key(key(K::Enter), &h, Path::new(".")), Out::Submit("git s\nñu".into()));
        assert!(e.buf.is_empty());
        assert_eq!(e.key(KeyEvent::new(K::Char('c'), M::CONTROL), &h, Path::new(".")), Out::Pass);
        assert_eq!(e.key(KeyEvent::new(K::Up, M::CONTROL), &h, Path::new(".")), Out::Blocks);
    }

    #[test]
    fn history_and_suggestion() {
        let mut h = History::load(None);
        for c in ["cargo build", "ls", "cargo test --release", "ls"] {
            h.push(c);
        }
        assert_eq!(h.items, ["cargo build", "cargo test --release", "ls"], "sin duplicados, el repetido al final");
        let mut e = Editor::default();
        typed(&mut e, "car", &h);
        assert_eq!(e.suggestion(&h), Some("go test --release"));
        e.key(key(K::Right), &h, Path::new("."));
        assert_eq!(e.buf, "cargo test --release");
        e.clear();
        typed(&mut e, "cargo", &h);
        e.key(key(K::Up), &h, Path::new("."));
        assert_eq!(e.buf, "cargo test --release");
        e.key(key(K::Up), &h, Path::new("."));
        assert_eq!(e.buf, "cargo build");
        e.key(key(K::Down), &h, Path::new("."));
        e.key(key(K::Down), &h, Path::new("."));
        assert_eq!(e.buf, "cargo", "vuelve al borrador");
        assert_eq!(unescape(&escape("a\\b\nc")), "a\\b\nc");
    }

    #[test]
    fn completes_paths() {
        let dir = std::env::temp_dir().join(format!("kp-ed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.join("Cargo.lock"), "").unwrap();
        let h = History::load(None);
        let mut e = Editor::default();
        typed(&mut e, "cat sr", &h);
        e.complete(&dir);
        assert_eq!(e.buf, format!("cat src{}", std::path::MAIN_SEPARATOR));
        e.set("cat car");
        e.complete(&dir);
        assert_eq!(e.buf, "cat Cargo.");
        assert_eq!(e.candidates.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
