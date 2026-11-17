use ratatui::style::Color;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Deserialize, Clone)]
pub struct Preset {
    pub name: String,
    /// Línea de comando, separada por espacios.
    // ponytail: split_whitespace, sin comillas; añadir shlex si hace falta un argumento con espacios.
    pub cmd: String,
    /// Los agentes reciben broadcast; los shells no.
    #[serde(default = "yes")]
    pub agent: bool,
}
fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(default)]
pub struct Config {
    pub presets: Vec<Preset>,
    /// Texto que, visible al final de la pantalla, indica que el agente espera al usuario.
    pub waiting_patterns: Vec<String>,
    /// Líneas de historial por sesión (controla la RAM).
    pub scrollback: usize,
    /// Pitido en el terminal cuando un agente en segundo plano termina o se bloquea.
    pub bell: bool,
    /// Nombre de un tema integrado (ver THEMES).
    pub theme: String,
    /// Colores "#rrggbb" que sobrescriben los del tema: accent, dim, sel_bg, bar_bg, working, ...
    pub colors: HashMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        let shell = if cfg!(windows) {
            "powershell.exe -NoLogo".to_string()
        } else {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
        };
        let p = |name: &str, cmd: &str, agent| Preset { name: name.into(), cmd: cmd.into(), agent };
        Config {
            presets: vec![
                p("shell", &shell, false),
                p("claude", "claude", true),
                p("codex", "codex", true),
                p("gemini", "gemini", true),
                p("aider", "aider", true),
            ],
            waiting_patterns: [
                "do you want", "(y/n)", "[y/n]", "❯ 1. yes", "allow once", "press enter to",
                "approve this", "waiting for your", "allow this", "allow command", "enter to confirm",
                "trust this folder", "yes, proceed",
            ]
            .map(String::from)
            .to_vec(),
            scrollback: 2000,
            bell: true,
            theme: THEMES[0].0.into(),
            colors: HashMap::new(),
        }
    }
}

/// Colores de la interfaz. La pantalla de cada sesión conserva los colores de su app.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub accent: Color,
    pub dim: Color,
    pub sel_bg: Color,
    pub bar_bg: Color,
    pub working: Color,
    pub waiting: Color,
    pub done: Color,
    pub exited: Color,
    pub add: Color,
    pub del: Color,
    /// Texto sobre fondos de color (botones, etiquetas).
    pub on_accent: Color,
}

/// accent, dim, sel_bg, bar_bg, working, waiting, done, exited, add, del, on_accent
pub const THEMES: &[(&str, [u32; 11])] = &[
    ("tokyo-night", [0x7aa2f7, 0x6e738c, 0x282c3e, 0x181a24, 0xe0af68, 0xf7768e, 0x9ece6a, 0x4a4f69, 0x9ece6a, 0xf7768e, 0x15161e]),
    ("catppuccin", [0xcba6f7, 0x7f849c, 0x313244, 0x181825, 0xf9e2af, 0xf38ba8, 0xa6e3a1, 0x585b70, 0xa6e3a1, 0xf38ba8, 0x11111b]),
    ("dracula", [0xbd93f9, 0x6272a4, 0x44475a, 0x21222c, 0xf1fa8c, 0xff5555, 0x50fa7b, 0x4d5066, 0x50fa7b, 0xff5555, 0x21222c]),
    ("gruvbox", [0xfabd2f, 0x928374, 0x3c3836, 0x1d2021, 0xfe8019, 0xfb4934, 0xb8bb26, 0x665c54, 0xb8bb26, 0xfb4934, 0x1d2021]),
    ("nord", [0x88c0d0, 0x616e88, 0x3b4252, 0x242933, 0xebcb8b, 0xbf616a, 0xa3be8c, 0x4c566a, 0xa3be8c, 0xbf616a, 0x242933]),
    ("light", [0x2e5bd8, 0x7a7f8c, 0xdde3f0, 0xeceff4, 0xb26b00, 0xc8323c, 0x2f8a3e, 0xa0a4ad, 0x2f8a3e, 0xc8323c, 0xffffff]),
];

fn rgb(h: u32) -> Color {
    Color::Rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

fn parse_hex(s: &str) -> Option<Color> {
    let h = s.strip_prefix('#')?;
    (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok().map(rgb)).flatten()
}

impl Default for Theme {
    fn default() -> Self {
        Theme::from_hex(THEMES[0].1)
    }
}

impl Theme {
    fn from_hex(c: [u32; 11]) -> Theme {
        let [accent, dim, sel_bg, bar_bg, working, waiting, done, exited, add, del, on_accent] = c.map(rgb);
        Theme { accent, dim, sel_bg, bar_bg, working, waiting, done, exited, add, del, on_accent }
    }

    fn slot(&mut self, name: &str) -> Option<&mut Color> {
        Some(match name {
            "accent" => &mut self.accent,
            "dim" => &mut self.dim,
            "sel_bg" => &mut self.sel_bg,
            "bar_bg" => &mut self.bar_bg,
            "working" => &mut self.working,
            "waiting" => &mut self.waiting,
            "done" => &mut self.done,
            "exited" => &mut self.exited,
            "add" => &mut self.add,
            "del" => &mut self.del,
            "on_accent" => &mut self.on_accent,
            _ => return None,
        })
    }

    /// Tema integrado `name` con los `colors` de la config encima.
    pub fn build(name: &str, colors: &HashMap<String, String>) -> Result<Theme, String> {
        let base = THEMES.iter().find(|(n, _)| *n == name).ok_or_else(|| {
            let names: Vec<_> = THEMES.iter().map(|t| t.0).collect();
            format!("tema '{name}' no existe ({})", names.join(", "))
        })?;
        let mut t = Theme::from_hex(base.1);
        for (k, v) in colors {
            let c = parse_hex(v).ok_or_else(|| format!("color '{k}' = '{v}': usa \"#rrggbb\""))?;
            *t.slot(k).ok_or_else(|| format!("color '{k}' no existe"))? = c;
        }
        Ok(t)
    }
}

pub fn path() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.unwrap_or_default().join("kodepeter").join("config.toml")
}

/// Carga la config; si el archivo no existe usa los valores por defecto. Devuelve el error de parseo, si lo hay.
pub fn load() -> (Config, Option<String>) {
    let p = path();
    match std::fs::read_to_string(&p) {
        Err(_) => (Config::default(), None),
        Ok(s) => match toml::from_str::<Config>(&s) {
            Ok(mut c) => {
                c.waiting_patterns.iter_mut().for_each(|w| *w = w.to_lowercase());
                c.scrollback = c.scrollback.min(100_000);
                if c.presets.is_empty() {
                    c.presets = Config::default().presets;
                }
                (c, None)
            }
            Err(e) => (Config::default(), Some(format!("{}: {}", p.display(), e.message()))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes() {
        let none = HashMap::new();
        for (name, _) in THEMES {
            Theme::build(name, &none).unwrap();
        }
        let over = HashMap::from([("accent".to_string(), "#ff0000".to_string())]);
        assert_eq!(Theme::build("nord", &over).unwrap().accent, Color::Rgb(255, 0, 0));
        assert!(Theme::build("nope", &none).is_err());
        let bad = HashMap::from([("accent".to_string(), "rojo".to_string())]);
        assert!(Theme::build("nord", &bad).is_err());
        let unknown = HashMap::from([("bogus".to_string(), "#000000".to_string())]);
        assert!(Theme::build("nord", &unknown).is_err());
    }
}
