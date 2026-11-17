//! Traduce eventos de teclado de crossterm a los bytes que espera una app dentro del PTY.
use ratatui::crossterm::event::{KeyCode as K, KeyEvent, KeyModifiers as M};

/// Tecla prefijo de kp: Ctrl+Space (sin Alt, para no confundir AltGr en teclados europeos).
pub fn is_prefix(k: &KeyEvent) -> bool {
    k.code == K::Null || (k.code == K::Char(' ') && k.modifiers.contains(M::CONTROL) && !k.modifiers.contains(M::ALT))
}

pub fn encode(k: &KeyEvent, app_cursor: bool) -> Vec<u8> {
    let m = k.modifiers;
    // AltGr llega como Ctrl+Alt: el carácter ya viene resuelto ('@', '#', '€'...).
    let altgr = m.contains(M::CONTROL | M::ALT);
    let ctrl = m.contains(M::CONTROL) && !altgr;
    let alt = m.contains(M::ALT) && !altgr;
    // Parámetro xterm de modificadores: 1 + shift + 2*alt + 4*ctrl.
    let modp = 1 + m.contains(M::SHIFT) as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let csi = |final_: char, ss3: bool| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[1;{modp}{final_}").into_bytes()
        } else if ss3 {
            format!("\x1bO{final_}").into_bytes()
        } else {
            format!("\x1b[{final_}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modp > 1 { format!("\x1b[{n};{modp}~") } else { format!("\x1b[{n}~") }.into_bytes()
    };
    let mut out = match k.code {
        K::Char(c) if ctrl => vec![match c.to_ascii_lowercase() {
            c @ 'a'..='z' => c as u8 & 0x1f,
            ' ' | '@' | '2' => 0,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' | '7' => 0x1f,
            '?' | '8' => 0x7f,
            c => return c.to_string().into_bytes(),
        }],
        K::Char(c) => c.to_string().into_bytes(),
        K::Null => vec![0],
        // Shift+Enter = nueva línea en Claude Code / Codex (equivale a Meta+Enter).
        K::Enter if m.contains(M::SHIFT) => b"\x1b\r".to_vec(),
        K::Enter => vec![b'\r'],
        K::Backspace if ctrl => vec![0x08],
        K::Backspace => vec![0x7f],
        K::Tab => vec![b'\t'],
        K::BackTab => b"\x1b[Z".to_vec(),
        K::Esc => vec![0x1b],
        K::Up => csi('A', app_cursor),
        K::Down => csi('B', app_cursor),
        K::Right => csi('C', app_cursor),
        K::Left => csi('D', app_cursor),
        K::Home => csi('H', app_cursor),
        K::End => csi('F', app_cursor),
        K::Insert => tilde(2),
        K::Delete => tilde(3),
        K::PageUp => tilde(5),
        K::PageDown => tilde(6),
        K::F(n @ 1..=4) => csi((b'P' + n - 1) as char, true),
        K::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5]),
        _ => return vec![],
    };
    // Alt como prefijo ESC solo para teclas "simples"; las CSI ya llevan el modificador.
    if alt && matches!(k.code, K::Char(_) | K::Backspace | K::Enter | K::Tab) {
        out.insert(0, 0x1b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(code: K, m: M) -> KeyEvent { KeyEvent::new(code, m) }

    #[test]
    fn encodes() {
        assert_eq!(encode(&key(K::Char('a'), M::NONE), false), b"a");
        assert_eq!(encode(&key(K::Char('c'), M::CONTROL), false), [3]);
        assert_eq!(encode(&key(K::Char('x'), M::ALT), false), b"\x1bx");
        assert_eq!(encode(&key(K::Char('@'), M::CONTROL | M::ALT), false), b"@"); // AltGr
        assert_eq!(encode(&key(K::Char('ñ'), M::NONE), false), "ñ".as_bytes());
        assert_eq!(encode(&key(K::Up, M::NONE), false), b"\x1b[A");
        assert_eq!(encode(&key(K::Up, M::NONE), true), b"\x1bOA");
        assert_eq!(encode(&key(K::Right, M::CONTROL), true), b"\x1b[1;5C");
        assert_eq!(encode(&key(K::Delete, M::NONE), false), b"\x1b[3~");
        assert_eq!(encode(&key(K::F(1), M::NONE), false), b"\x1bOP");
        assert_eq!(encode(&key(K::F(12), M::NONE), false), b"\x1b[24~");
        assert_eq!(encode(&key(K::Enter, M::SHIFT), false), b"\x1b\r");
        assert_eq!(encode(&key(K::BackTab, M::SHIFT), false), b"\x1b[Z");
        assert!(is_prefix(&key(K::Char(' '), M::CONTROL)));
        assert!(!is_prefix(&key(K::Char(' '), M::NONE)));
    }
}
