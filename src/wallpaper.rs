//! Fondo de pantalla dentro de la terminal: la imagen tiñe el color de fondo de las celdas que usan el fondo
//! por defecto, mezclada con el fondo del tema para que el texto siga legible. Nunca cambia caracteres (el
//! texto se sigue copiando y leyendo igual). Sin protocolos de imagen: basta con color real.
use crate::Event;
use image::{DynamicImage, RgbImage, imageops::FilterType};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// Resolución máxima que se guarda en memoria (~700 KB); más no se ve en una terminal.
const MAX_W: u32 = 640;
const MAX_H: u32 = 360;

pub struct Wallpaper {
    pub path: PathBuf,
    img: DynamicImage,
    /// Píxeles escalados para el último tamaño pintado: (ancho, alto en píxeles) → imagen.
    cache: Option<((u32, u32), RgbImage)>,
}

/// Decodifica y reduce la imagen (fuera del hilo de la interfaz).
pub fn decode(path: &Path) -> Result<Wallpaper, String> {
    let img = image::open(path).map_err(|e| format!("fondo '{}': {e}", path.display()))?;
    Ok(Wallpaper { path: path.to_path_buf(), img: DynamicImage::ImageRgb8(img.thumbnail(MAX_W, MAX_H).to_rgb8()), cache: None })
}

/// Carga en segundo plano; el resultado llega como `Event::Wallpaper`.
pub fn load_async(path: PathBuf, tx: Sender<Event>) {
    std::thread::spawn(move || {
        let _ = tx.send(Event::Wallpaper(decode(&path).map(Box::new)));
    });
}

fn rgb(c: Color) -> [u8; 3] {
    match c {
        Color::Rgb(r, g, b) => [r, g, b],
        _ => [0, 0, 0],
    }
}

fn mix(base: [u8; 3], px: [u8; 3], opacity: f32) -> Color {
    let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * opacity).round() as u8;
    Color::Rgb(m(base[0], px[0]), m(base[1], px[1]), m(base[2], px[2]))
}

impl Wallpaper {
    fn pixels(&mut self, w: u32, h: u32) -> &RgbImage {
        if self.cache.as_ref().is_none_or(|(size, _)| *size != (w, h)) {
            // "Cover": llena el área recortando lo que sobra, sin deformar.
            let scaled = self.img.resize_to_fill(w.max(1), h.max(1), FilterType::Triangle).to_rgb8();
            self.cache = Some(((w, h), scaled));
        }
        &self.cache.as_ref().unwrap().1
    }

    /// Pinta la imagen en `area` sobre las celdas cuyo fondo es `bg` (el del tema).
    pub fn paint(&mut self, buf: &mut Buffer, area: Rect, bg: Color, opacity: f32) {
        let base = rgb(bg);
        let px = self.pixels(area.width as u32, area.height as u32 * 2);
        for y in 0..area.height {
            for x in 0..area.width {
                let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) else { continue };
                if cell.bg != bg {
                    continue;
                }
                // Una celda mide ~1x2: se escala a dos píxeles por celda y se promedian (sin deformar).
                let top = px.get_pixel(x as u32, y as u32 * 2).0;
                let bottom = px.get_pixel(x as u32, y as u32 * 2 + 1).0;
                let avg = [0, 1, 2].map(|i| ((top[i] as u16 + bottom[i] as u16) / 2) as u8);
                cell.set_bg(mix(base, avg, opacity));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    #[test]
    fn paints_only_default_background() {
        let path = std::env::temp_dir().join(format!("kp-wall-{}.png", std::process::id()));
        // Mitad superior roja, inferior azul.
        let img = RgbImage::from_fn(40, 20, |_, y| if y < 10 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) });
        img.save(&path).unwrap();
        let mut w = decode(&path).unwrap();
        let bg = Color::Rgb(0, 0, 0);
        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);
        buf.set_style(area, Style::new().bg(bg));
        buf.set_string(0, 1, "x", Style::new().bg(bg));
        buf[(3, 1)].set_bg(Color::Rgb(9, 9, 9)); // fondo propio de una app: no se toca
        w.paint(&mut buf, area, bg, 0.5);
        assert_eq!(buf[(0, 0)].symbol(), " ", "no cambia caracteres");
        let Color::Rgb(r0, _, b0) = buf[(0, 0)].bg else { panic!("sin fondo") };
        assert!(r0 > b0 && r0 <= 128, "arriba: predomina el rojo, atenuado ({r0}, {b0})");
        assert_eq!(buf[(0, 1)].symbol(), "x", "el texto se conserva");
        let Color::Rgb(r, _, b) = buf[(0, 1)].bg else { panic!("sin fondo") };
        assert!(b > r && b <= 128, "abajo: predomina el azul, atenuado ({r}, {b})");
        assert_eq!(buf[(3, 1)].bg, Color::Rgb(9, 9, 9));
        assert!(decode(Path::new("no-existe.png")).is_err());
        let _ = std::fs::remove_file(path);
    }
}
