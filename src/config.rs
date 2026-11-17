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
    /// Usa el fondo de tu terminal (acrílico, imagen) en vez del fondo del tema.
    pub transparent: bool,
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
            transparent: false,
        }
    }
}

/// Entorno visual completo: interfaz + fondo/texto + paleta ANSI aplicada a las terminales.
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
    /// Fondo y texto por defecto de kp y de las sesiones (`Color::Reset` = transparente).
    pub bg: Color,
    pub fg: Color,
    /// Los 16 colores ANSI que usan las apps dentro de las sesiones.
    pub ansi: [Color; 16],
}

/// (nombre, [accent, dim, sel_bg, bar_bg, working, waiting, done, exited, add, del, on_accent, bg, fg], ansi 0-15)
type ThemeDef = (&'static str, [u32; 13], [u32; 16]);

pub const THEMES: &[ThemeDef] = &[
    (
        "tokyo-night",
        [0x7aa2f7, 0x6e738c, 0x292e42, 0x16161e, 0xe0af68, 0xf7768e, 0x9ece6a, 0x4a4f69, 0x9ece6a, 0xf7768e, 0x15161e, 0x1a1b26, 0xc0caf5],
        [0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5],
    ),
    (
        "catppuccin",
        [0xcba6f7, 0x7f849c, 0x313244, 0x181825, 0xf9e2af, 0xf38ba8, 0xa6e3a1, 0x585b70, 0xa6e3a1, 0xf38ba8, 0x11111b, 0x1e1e2e, 0xcdd6f4],
        [0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, 0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8],
    ),
    (
        "dracula",
        [0xbd93f9, 0x6272a4, 0x44475a, 0x21222c, 0xf1fa8c, 0xff5555, 0x50fa7b, 0x4d5066, 0x50fa7b, 0xff5555, 0x21222c, 0x282a36, 0xf8f8f2],
        [0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2, 0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff],
    ),
    (
        "gruvbox",
        [0xfabd2f, 0x928374, 0x3c3836, 0x1d2021, 0xfe8019, 0xfb4934, 0xb8bb26, 0x665c54, 0xb8bb26, 0xfb4934, 0x1d2021, 0x282828, 0xebdbb2],
        [0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984, 0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2],
    ),
    (
        "nord",
        [0x88c0d0, 0x616e88, 0x3b4252, 0x242933, 0xebcb8b, 0xbf616a, 0xa3be8c, 0x4c566a, 0xa3be8c, 0xbf616a, 0x242933, 0x2e3440, 0xd8dee9],
        [0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0, 0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4],
    ),
    (
        "one-dark",
        [0x61afef, 0x5c6370, 0x2c313c, 0x21252b, 0xe5c07b, 0xe06c75, 0x98c379, 0x4b5263, 0x98c379, 0xe06c75, 0x21252b, 0x282c34, 0xabb2bf],
        [0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff],
    ),
    (
        "rose-pine",
        [0xc4a7e7, 0x6e6a86, 0x26233a, 0x1f1d2e, 0xf6c177, 0xeb6f92, 0x9ccfd8, 0x524f67, 0x9ccfd8, 0xeb6f92, 0x191724, 0x191724, 0xe0def4],
        [0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4, 0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4],
    ),
    (
        "light",
        [0x0969da, 0x6e7781, 0xdde7f5, 0xf0f3f6, 0x9a6700, 0xcf222e, 0x1a7f37, 0xa0a6ad, 0x1a7f37, 0xcf222e, 0xffffff, 0xffffff, 0x24292f],
        [0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781, 0x57606a, 0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f],
    ),
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
        Theme::from_def(&THEMES[0])
    }
}

impl Theme {
    fn from_def(d: &ThemeDef) -> Theme {
        let [accent, dim, sel_bg, bar_bg, working, waiting, done, exited, add, del, on_accent, bg, fg] = d.1.map(rgb);
        Theme { accent, dim, sel_bg, bar_bg, working, waiting, done, exited, add, del, on_accent, bg, fg, ansi: d.2.map(rgb) }
    }

    fn slot(&mut self, name: &str) -> Option<&mut Color> {
        if let Some(i) = name.strip_prefix("ansi").and_then(|n| n.parse::<usize>().ok()) {
            return self.ansi.get_mut(i);
        }
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
            "bg" => &mut self.bg,
            "fg" => &mut self.fg,
            _ => return None,
        })
    }

    /// Tema integrado `name` con los `colors` de la config encima.
    pub fn build(name: &str, colors: &HashMap<String, String>) -> Result<Theme, String> {
        let base = THEMES.iter().find(|d| d.0 == name).ok_or_else(|| {
            let names: Vec<_> = THEMES.iter().map(|t| t.0).collect();
            format!("tema '{name}' no existe ({})", names.join(", "))
        })?;
        let mut t = Theme::from_def(base);
        for (k, v) in colors {
            let c = parse_hex(v).ok_or_else(|| format!("color '{k}' = '{v}': usa \"#rrggbb\""))?;
            *t.slot(k).ok_or_else(|| format!("color '{k}' no existe"))? = c;
        }
        Ok(t)
    }

    /// Color de una celda de la sesión, traducido al entorno visual.
    pub fn map(&self, c: vt100::Color, default: Color) -> Color {
        match c {
            vt100::Color::Default => default,
            vt100::Color::Idx(i) if i < 16 => self.ansi[i as usize],
            vt100::Color::Idx(i) => Color::Indexed(i),
            vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
        }
    }
}

impl Config {
    /// Tema `name` con los colores y la transparencia de la config.
    pub fn theme_named(&self, name: &str) -> Result<Theme, String> {
        let mut t = Theme::build(name, &self.colors)?;
        if self.transparent {
            t.bg = Color::Reset;
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
        for (name, ..) in THEMES {
            Theme::build(name, &none).unwrap();
        }
        let ansi = HashMap::from([("ansi1".to_string(), "#010203".to_string())]);
        assert_eq!(Theme::build("nord", &ansi).unwrap().ansi[1], Color::Rgb(1, 2, 3));
        let t = Theme::default();
        assert_eq!(t.map(vt100::Color::Idx(4), Color::Reset), t.ansi[4]);
        assert_eq!(t.map(vt100::Color::Default, t.bg), t.bg);
        assert_eq!(t.map(vt100::Color::Idx(200), t.bg), Color::Indexed(200));
        let over = HashMap::from([("accent".to_string(), "#ff0000".to_string())]);
        assert_eq!(Theme::build("nord", &over).unwrap().accent, Color::Rgb(255, 0, 0));
        assert!(Theme::build("nope", &none).is_err());
        let bad = HashMap::from([("accent".to_string(), "rojo".to_string())]);
        assert!(Theme::build("nord", &bad).is_err());
        let unknown = HashMap::from([("bogus".to_string(), "#000000".to_string())]);
        assert!(Theme::build("nord", &unknown).is_err());
    }
}
