use crate::config::Theme;
use crate::session::{Session, Status};
use crate::{App, Mode};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use std::path::Path;
use std::time::Duration;

pub const SIDEBAR: u16 = 30;
/// Filas reservadas para el editor de comandos en shells con bloques.
pub const EDITOR_H: u16 = 3;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn grid_cols(n: usize) -> usize {
    (n as f64).sqrt().ceil().max(1.0) as usize
}

fn icon(s: &Session, t: &Theme, frame: usize) -> (&'static str, Color) {
    match s.status {
        Status::Working => (SPINNER[frame % SPINNER.len()], t.working),
        Status::Waiting => ("▲", t.waiting),
        Status::Idle if s.attention => ("✔", t.done),
        Status::Idle => ("○", t.dim),
        Status::Exited => ("✕", t.exited),
    }
}

fn elapsed(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

/// Ruta con la carpeta de usuario abreviada como `~`.
fn short_path(p: &Path) -> String {
    let full = p.display().to_string();
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    match full.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => full,
    }
}

/// Ancho en celdas de terminal (los emoji y CJK ocupan 2).
fn cw(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

/// Recorta a `n` celdas sin partir grafemas, con «…» si sobra.
fn truncate(s: &str, n: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    if cw(s) <= n {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for g in s.graphemes(true) {
        let w = cw(g);
        if used + w + 1 > n {
            break;
        }
        used += w;
        out.push_str(g);
    }
    out + "…"
}

/// Copia la pantalla emulada al buffer, traduciendo sus colores al entorno visual del tema.
fn draw_screen(buf: &mut Buffer, area: Rect, screen: &vt100::Screen, top: u16, t: &Theme) {
    for y in 0..area.height {
        for x in 0..area.width {
            let Some(out) = buf.cell_mut((area.x + x, area.y + y)) else { continue };
            match screen.cell(top + y, x) {
                Some(c) if c.is_wide_continuation() => {
                    out.set_symbol("");
                }
                Some(c) => {
                    let mut m = Modifier::empty();
                    m.set(Modifier::BOLD, c.bold());
                    m.set(Modifier::DIM, c.dim());
                    m.set(Modifier::ITALIC, c.italic());
                    m.set(Modifier::UNDERLINED, c.underline());
                    m.set(Modifier::REVERSED, c.inverse());
                    out.set_symbol(if c.has_contents() { c.contents() } else { " " });
                    out.set_style(
                        Style::new().fg(t.map(c.fgcolor(), t.fg)).bg(t.map(c.bgcolor(), t.bg)).add_modifier(m),
                    );
                }
                None => {
                    out.set_symbol(" ");
                    out.set_style(Style::new().bg(t.bg));
                }
            }
        }
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    // Al recorrer temas en la paleta se previsualizan en vivo.
    let t = app.preview_theme().unwrap_or(app.theme);
    // Movimiento reducido: sin animación, el spinner queda fijo.
    let frame = if app.cfg.animations { (app.started.elapsed().as_millis() / 100) as usize } else { 0 };
    f.render_widget(Block::new().style(Style::new().bg(t.bg).fg(t.fg)), f.area());

    let [side, right] = Layout::horizontal([Constraint::Length(app.sidebar_w()), Constraint::Min(1)]).areas(f.area());
    let [tabs_row, main, bar] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(2), Constraint::Length(1)]).areas(right);
    // Tamaño estable para los agentes, sin importar qué vista esté encima.
    app.resize(main.height, main.width);

    app.hit_new = if side.width > 0 { sidebar(f, side, app, &t, frame) } else { Rect::default() };
    (app.hit_tabs, app.hit_plus) = tabs(f, tabs_row, app, &t, frame);
    app.hit_body = Rect::default();
    if let Some(g) = app.git.as_mut() {
        g.draw(f, main, &t);
    } else if app.grid {
        grid(f, main, app, &t, frame);
    } else if app.sessions.get(app.focus).is_some() {
        app.hit_body = session_view(f, main, app, &t, frame);
    } else {
        welcome(f, main, &t);
    }
    status_bar(f, bar, app, &t);
    // Fondo de pantalla: solo en el área de contenido y en celdas con el fondo del tema, antes de los overlays.
    if let Some(w) = app.wallpaper.as_mut() {
        w.paint(f.buffer_mut(), main, t.bg, app.wall_opacity);
    }
    match app.mode {
        Mode::Log => log_view(f, main, app, &t),
        Mode::Palette => palette(f, main, app, &t),
        Mode::Prefix => which_key(f, main, &t),
        _ => {}
    }
    app.hit_toasts = toasts(f, main, app, &t);
}

/// Pestañas estilo Warp + información de la sesión enfocada a la derecha. Devuelve zonas clicables.
fn tabs(f: &mut Frame, area: Rect, app: &App, t: &Theme, frame: usize) -> (Vec<(Rect, usize)>, Rect) {
    f.render_widget(Block::new().style(Style::new().bg(t.bar_bg)), area);
    let info = app.sessions.get(app.focus).map(|s| {
        format!(
            " {}{}  ·  {} ",
            s.branch.as_ref().map_or(String::new(), |b| format!("⎇ {b}  ·  ")),
            short_path(&s.cwd),
            elapsed(s.since.elapsed())
        )
    });
    let info_w = info.as_ref().map_or(0, |i| cw(i) as u16);
    let room = area.width.saturating_sub(if info_w < area.width / 2 { info_w } else { 0 });
    let mut hits = vec![];
    let mut x = area.x;
    for (i, s) in app.sessions.iter().enumerate() {
        let (ic, col) = icon(s, t, frame);
        let label = format!(" {ic} {} {} ", i + 1, truncate(&s.name, 16));
        let w = cw(&label) as u16 + 1;
        if x + w + 4 > area.x + room {
            break;
        }
        let active = i == app.focus;
        let r = Rect::new(x, area.y, w, 1);
        let name_fg = if s.attention { t.done } else if active { t.fg } else { t.dim };
        let mut style = Style::new().fg(name_fg);
        if active {
            style = style.bg(t.bg).add_modifier(Modifier::BOLD);
        }
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if active { "▎" } else { " " }, style.fg(t.accent)),
                Span::styled(format!(" {ic}"), style.fg(col)),
                Span::styled(format!(" {} {} ", i + 1, truncate(&s.name, 16)), style),
            ])),
            r,
        );
        hits.push((r, i));
        x += w;
    }
    let plus = Rect::new(x, area.y, 3.min(area.right().saturating_sub(x)), 1);
    f.render_widget(Paragraph::new(" + ").style(Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), plus);
    if let Some(info) = info.filter(|_| info_w < area.width / 2) {
        f.render_widget(
            Paragraph::new(info).style(Style::new().fg(t.dim)),
            Rect::new(area.right() - info_w, area.y, info_w, 1),
        );
    }
    (hits, plus)
}

/// Terminal de la sesión enfocada. En shells integrados: margen de bloques, etiquetas de estado y editor.
/// Devuelve la zona de la rejilla de la terminal.
fn session_view(f: &mut Frame, area: Rect, app: &App, t: &Theme, frame: usize) -> Rect {
    let s = &app.sessions[app.focus];
    let scr = s.parser.screen();
    if !s.integrated {
        draw_screen(f.buffer_mut(), area, scr, 0, t);
        if s.scroll == 0 && !scr.hide_cursor() && app.mode == Mode::Normal {
            let (r, c) = scr.cursor_position();
            f.set_cursor_position(Position::new(area.x + c, area.y + r));
        }
        scroll_tag(f, area, s, t);
        return area;
    }
    let eh = EDITOR_H.min(area.height.saturating_sub(1));
    let grid = Rect::new(area.x + 1, area.y, area.width.saturating_sub(1), area.height - eh);
    let ed = Rect::new(area.x, grid.bottom(), area.width, eh);
    draw_screen(f.buffer_mut(), grid, scr, 0, t);

    // Bloques: margen de color por resultado, cabecera tintada con ✔/✘ y duración, selección resaltada.
    let top = s.top_line();
    for y in 0..grid.height {
        let line = top + y as u64;
        let Some(i) = s.block_at(line) else { continue };
        let b = &s.blocks[i];
        if b.cmd.is_empty() {
            continue;
        }
        let selected = app.block_sel == Some(i);
        let col = match b.exit {
            _ if b.running() => t.working,
            Some(0) => t.done,
            Some(_) => t.waiting,
            None => t.dim,
        };
        let buf = f.buffer_mut();
        if let Some(c) = buf.cell_mut((area.x, grid.y + y)) {
            c.set_symbol(if selected { "┃" } else { "▎" }).set_fg(if selected { t.accent } else { col }).set_bg(t.bg);
        }
        let header = line == b.prompt;
        if header || selected {
            let tint = if selected { t.sel_bg } else { t.bar_bg };
            for x in grid.x..grid.right() {
                if let Some(c) = buf.cell_mut((x, grid.y + y)) {
                    if c.bg == t.bg {
                        c.set_bg(tint);
                    }
                }
            }
        }
        if header {
            let badge = match (b.running(), b.exit, b.dur) {
                (true, ..) => format!(" {} {} ", SPINNER[frame % SPINNER.len()], elapsed(b.started.map_or(Duration::ZERO, |s| s.elapsed()))),
                (false, Some(0), Some(d)) => format!(" ✔ {} ", fmt_dur(d)),
                (false, Some(code), Some(d)) => format!(" ✘ {code} · {} ", fmt_dur(d)),
                (false, None, Some(d)) => format!(" {} ", fmt_dur(d)),
                // Tecleado directamente en el shell: resultado sin duración.
                (false, Some(0), None) => " ✔ ".into(),
                (false, Some(code), None) => format!(" ✘ {code} "),
                _ => String::new(),
            };
            let w = cw(&badge) as u16;
            if w > 0 && w < grid.width {
                f.render_widget(
                    Paragraph::new(badge).style(Style::new().fg(col).bg(if selected { t.sel_bg } else { t.bar_bg }).add_modifier(Modifier::BOLD)),
                    Rect::new(grid.right() - w, grid.y + y, w, 1),
                );
            }
        }
    }
    // Como en Warp, el prompt vivo del shell no se muestra: se escribe en el editor de abajo.
    if s.editor_active() {
        if let Some(live) = s.blocks.last().filter(|b| b.cmd.is_empty()) {
            let from = live.prompt.saturating_sub(top).min(grid.height as u64) as u16;
            let buf = f.buffer_mut();
            for y in from..grid.height {
                for x in grid.x..grid.right() {
                    if let Some(c) = buf.cell_mut((x, grid.y + y)) {
                        c.set_symbol(" ").set_style(Style::new().bg(t.bg));
                    }
                }
            }
        }
    }
    scroll_tag(f, grid, s, t);
    editor_box(f, ed, grid, app, s, t, frame);
    grid
}

fn fmt_dur(d: Duration) -> String {
    if d.as_secs() >= 60 { elapsed(d) } else { format!("{:.1}s", d.as_secs_f32()) }
}

fn scroll_tag(f: &mut Frame, area: Rect, s: &Session, t: &Theme) {
    if s.scroll > 0 {
        let tag = format!(" ↑ historial {} · cualquier tecla vuelve ", s.scroll);
        let w = (cw(&tag) as u16).min(area.width);
        f.render_widget(
            Paragraph::new(tag).style(Style::new().bg(t.accent).fg(t.on_accent).add_modifier(Modifier::BOLD)),
            Rect::new(area.right() - w, area.y, w, 1),
        );
    }
}

/// Resaltado sencillo: comando en acento, opciones atenuadas, cadenas en verde.
fn highlight<'a>(line: &'a str, first_line: bool, t: &Theme) -> Vec<Span<'a>> {
    let mut spans = vec![];
    let mut quote: Option<char> = None;
    let mut first = first_line;
    let mut start = 0;
    let flush = |spans: &mut Vec<Span<'a>>, seg: &'a str, first: &mut bool, quoted: bool| {
        if seg.is_empty() {
            return;
        }
        let style = if quoted {
            Style::new().fg(t.done)
        } else if *first && !seg.trim().is_empty() {
            *first = false;
            Style::new().fg(t.accent).add_modifier(Modifier::BOLD)
        } else if seg.starts_with('-') {
            Style::new().fg(t.dim)
        } else {
            Style::new().fg(t.fg)
        };
        spans.push(Span::styled(seg, style));
    };
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) if c == q => {
                flush(&mut spans, &line[start..i + 1], &mut first, true);
                start = i + 1;
                quote = None;
            }
            Some(_) => {}
            None if c == '"' || c == '\'' => {
                flush(&mut spans, &line[start..i], &mut first, false);
                start = i;
                quote = Some(c);
            }
            None if c == ' ' => {
                flush(&mut spans, &line[start..i], &mut first, false);
                spans.push(Span::raw(" "));
                start = i + 1;
            }
            None => {}
        }
    }
    flush(&mut spans, &line[start..], &mut first, quote.is_some());
    spans
}

/// Caja del editor (estilo Warp). Crece hacia arriba con comandos multilínea.
#[allow(clippy::too_many_arguments)]
fn editor_box(f: &mut Frame, area: Rect, grid: Rect, app: &App, s: &Session, t: &Theme, frame: usize) {
    let ed = &s.editor;
    let active = s.editor_active() && app.block_sel.is_none();
    let lines: Vec<&str> = if ed.buf.is_empty() { vec![""] } else { ed.buf.split('\n').collect() };
    let h = (lines.len() as u16 + 2).max(area.height).min(area.height + grid.height / 2);
    let r = Rect::new(area.x, area.bottom() - h, area.width, h);
    f.render_widget(Clear, r);
    let title = format!(" {}{} ", short_path(&s.cwd), s.branch.as_ref().map_or(String::new(), |b| format!("  ⎇ {b}")));
    // Ayuda adaptable: la versión más larga que quepa junto al título (en terminales estrechas, ninguna).
    let hints: &[&str] = if app.block_sel.is_some() {
        &[" ↑↓ bloques · c copiar · o salida · r reutilizar · Esc volver ", " c copiar · r reutilizar · Esc ", ""]
    } else {
        &[" Enter ejecutar · ⇧Enter línea · ↑ historial · → sugerencia · Tab completar · Ctrl+↑ bloques ", " Tab completar · Ctrl+↑ bloques ", ""]
    };
    let room = (r.width as usize).saturating_sub(4);
    let hint = *hints.iter().find(|h| cw(h) <= room).unwrap_or(&"");
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if active { t.accent } else { t.dim }))
        .style(Style::new().bg(t.bar_bg))
        .title(Span::styled(title, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
        .title_bottom(Line::styled(hint, Style::new().fg(t.dim)).right_aligned());
    let inner = block.inner(r);
    f.render_widget(block, r);

    if !active && app.block_sel.is_none() {
        // Un comando está corriendo: las teclas van al programa.
        let running = s.blocks.iter().rev().find(|b| b.running());
        let text = match running {
            Some(b) => format!(
                " {} ejecutando  {}  · {} · las teclas van al programa · Ctrl+C cancela",
                SPINNER[frame % SPINNER.len()],
                truncate(&b.cmd, 40),
                elapsed(b.started.map_or(Duration::ZERO, |s| s.elapsed()))
            ),
            None => " la app tiene el control de la terminal · las teclas van al programa".into(),
        };
        f.render_widget(Paragraph::new(text).style(Style::new().fg(t.working)), inner);
        return;
    }

    // Texto con resaltado; desplazamiento horizontal si el cursor se sale.
    let before = &ed.buf[..ed.cur];
    let cur_row = before.matches('\n').count();
    let cur_col = cw(before.rsplit('\n').next().unwrap_or("")) as u16;
    let prompt_w = 2u16;
    let avail = inner.width.saturating_sub(prompt_w + 1);
    let hscroll = cur_col.saturating_sub(avail) as usize;
    let mut out = vec![];
    for (i, l) in lines.iter().enumerate() {
        let shown: &str = if i == cur_row && hscroll > 0 {
            let off = l.char_indices().nth(hscroll).map_or(l.len(), |(b, _)| b);
            &l[off..]
        } else {
            l
        };
        let mut spans = vec![Span::styled(if i == 0 { "❯ " } else { "  " }, Style::new().fg(t.accent).add_modifier(Modifier::BOLD))];
        spans.extend(highlight(shown, i == 0, t));
        if i == lines.len() - 1 {
            if let Some(sug) = ed.suggestion(&app.history) {
                spans.push(Span::styled(sug, Style::new().fg(t.dim)));
            }
        }
        out.push(Line::from(spans));
    }
    if ed.buf.is_empty() {
        out[0].push_span(Span::styled("escribe un comando…  (↑ historial · Tab completa · ^Space p paleta)", Style::new().fg(t.dim)));
    }
    let skip = (cur_row + 1).saturating_sub(inner.height as usize);
    f.render_widget(Paragraph::new(out.into_iter().skip(skip).collect::<Vec<_>>()), inner);
    if app.mode == Mode::Normal && app.block_sel.is_none() {
        f.set_cursor_position(Position::new(
            inner.x + prompt_w + cur_col - hscroll as u16,
            inner.y + (cur_row - skip) as u16,
        ));
    }

    // Opciones de autocompletado, encima de la caja.
    if !ed.candidates.is_empty() {
        let shown: Vec<&String> = ed.candidates.iter().take(8).collect();
        let w = (shown.iter().map(|c| cw(c)).max().unwrap_or(0) as u16 + 4).min(r.width);
        let hh = shown.len() as u16 + 2;
        let pr = Rect::new(r.x + 2, r.y.saturating_sub(hh), w, hh);
        f.render_widget(Clear, pr);
        let more = ed.candidates.len().saturating_sub(shown.len());
        let pb = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(t.accent))
            .style(Style::new().bg(t.bar_bg))
            .title_bottom(Span::styled(if more > 0 { format!(" +{more} ") } else { String::new() }, Style::new().fg(t.dim)));
        let pi = pb.inner(pr);
        f.render_widget(pb, pr);
        f.render_widget(
            Paragraph::new(shown.iter().map(|c| Line::styled(format!(" {c}"), Style::new().fg(t.fg))).collect::<Vec<_>>()),
            pi,
        );
    }
}

/// Devuelve la zona del botón «+ nuevo agente».
fn sidebar(f: &mut Frame, area: Rect, app: &App, t: &Theme, frame: usize) -> Rect {
    let count = |st: Status| app.sessions.iter().filter(|s| s.status == st).count();
    let summary = Line::from(vec![
        Span::styled(format!(" ●{} ", count(Status::Working)), Style::new().fg(t.working)),
        Span::styled(format!("▲{} ", count(Status::Waiting)), Style::new().fg(t.waiting)),
        Span::styled(format!("✔{} ", app.sessions.iter().filter(|s| s.attention).count()), Style::new().fg(t.done)),
    ]);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.dim))
        .style(Style::new().bg(t.bar_bg))
        .title(Span::styled(" ◆ kp ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
        .title_bottom(summary);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let w = inner.width as usize;
    let mut lines = vec![];
    for (i, s) in app.sessions.iter().enumerate() {
        let (ic, col) = icon(s, t, frame);
        let sel = i == app.focus;
        let bg = if sel { Style::new().bg(t.sel_bg) } else { Style::new() };
        let name = Style::new().fg(if s.attention { t.done } else { t.fg }).add_modifier(Modifier::BOLD);
        let mut first = Line::from(vec![
            Span::styled(if sel { "▌" } else { " " }, Style::new().fg(t.accent)),
            Span::styled(ic, Style::new().fg(col)),
            Span::styled(format!(" {} ", i + 1), Style::new().fg(t.dim)),
            Span::styled(s.name.clone(), name),
            Span::styled(format!(" {}", s.status.label()), Style::new().fg(col)),
        ]);
        let clock = elapsed(s.since.elapsed());
        let pad = w.saturating_sub(first.width() + clock.len() + 1);
        first.push_span(Span::raw(" ".repeat(pad)));
        first.push_span(Span::styled(clock, Style::new().fg(t.dim)));
        lines.push(first.style(bg));

        // Los agentes ponen en el título la tarea actual; el de un shell suele ser la ruta del .exe.
        let task = if !s.agent || s.title().is_empty() {
            s.cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        } else {
            s.title().to_string()
        };
        let sub = match &s.branch {
            Some(b) => format!("{task}  ⎇ {b}"),
            None => task,
        };
        lines.push(Line::styled(format!("    {}", truncate(&sub, w.saturating_sub(4))), bg.fg(t.dim)));
    }
    f.render_widget(Paragraph::new(lines), inner);

    if inner.height as usize > app.sessions.len() * 2 + 1 {
        let r = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
        let label = format!(" + nuevo agente{:>w$} ", "^Space p", w = w.saturating_sub(17));
        f.render_widget(
            Paragraph::new(label).style(Style::new().bg(t.sel_bg).fg(t.accent).add_modifier(Modifier::BOLD)),
            r,
        );
        return r;
    }
    Rect::default()
}

fn grid(f: &mut Frame, area: Rect, app: &App, t: &Theme, frame: usize) {
    let n = app.sessions.len();
    if n == 0 {
        return welcome(f, area, t);
    }
    let cols = grid_cols(n);
    let rows = n.div_ceil(cols);
    let (w, h) = (area.width / cols as u16, area.height / rows as u16);
    for (i, s) in app.sessions.iter().enumerate() {
        let tile = Rect::new(area.x + (i % cols) as u16 * w, area.y + (i / cols) as u16 * h, w, h);
        let (ic, col) = icon(s, t, frame);
        let focused = i == app.focus;
        let block = Block::bordered()
            .border_type(if focused { BorderType::Thick } else { BorderType::Rounded })
            .border_style(Style::new().fg(if focused { t.accent } else { col }))
            .title(Line::from(vec![
                Span::styled(format!(" {ic} "), Style::new().fg(col)),
                Span::styled(format!("{} {} ", i + 1, s.name), Style::new().fg(t.fg).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{} ", s.status.label()), Style::new().fg(col)),
            ]))
            .title_bottom(Span::styled(format!(" {} ", elapsed(s.since.elapsed())), Style::new().fg(t.dim)));
        let inner = block.inner(tile);
        f.render_widget(block, tile);
        // Muestra las últimas filas con contenido, sin redimensionar al agente.
        let scr = s.parser.screen();
        let last = scr.rows(0, scr.size().1.get()).collect::<Vec<_>>().iter().rposition(|r| !r.trim().is_empty());
        let top = last.map_or(0, |l| (l as u16 + 1).saturating_sub(inner.height));
        draw_screen(f.buffer_mut(), inner, scr, top, t);
    }
}

fn welcome(f: &mut Frame, area: Rect, t: &Theme) {
    let key = Style::new().fg(t.accent).add_modifier(Modifier::BOLD);
    let logo = Style::new().fg(t.accent).add_modifier(Modifier::BOLD);
    let mut lines = vec![
        Line::styled("█▄▀ █▀█", logo),
        Line::styled("█ █ █▀▀", logo),
        Line::raw(""),
        Line::styled("kodepeter · terminal para agentes de desarrollo", Style::new().fg(t.dim)),
        Line::raw(""),
    ];
    for (k, label) in [
        ("Ctrl+Space p", "lanzar un agente o una acción"),
        ("Ctrl+Space n", "shell nuevo"),
        ("Ctrl+Space r", "revisar cambios con git"),
        ("Ctrl+Space g", "ver todos los agentes"),
    ] {
        lines.push(Line::from(vec![Span::styled(format!("{k:>14}  "), key), Span::styled(format!("{label:<30}"), Style::new().fg(t.fg))]));
    }
    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    f.render_widget(
        Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center),
        Rect::new(area.x, y, area.width, h.min(area.height)),
    );
}

fn status_bar(f: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let editor = app.sessions.get(app.focus).is_some_and(|s| s.editor_active());
    let (mode, col) = match app.mode {
        Mode::Normal if app.block_sel.is_some() => ("BLOQUES", t.accent),
        Mode::Prefix => ("PREFIJO", t.working),
        Mode::Palette => ("PALETA", t.accent),
        Mode::Broadcast => ("BROADCAST", t.waiting),
        Mode::Wallpaper => ("FONDO", t.accent),
        Mode::Log => ("REGISTRO", t.accent),
        Mode::Normal if app.git.is_some() => ("GIT", t.done),
        Mode::Normal if app.grid => ("GRID", t.accent),
        Mode::Normal => ("KP", t.accent),
    };
    let mut spans = vec![
        Span::styled(format!(" {mode} "), Style::new().bg(col).fg(t.on_accent).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
    ];
    let hint = |k: &str, l: &str| {
        [Span::styled(k.to_string(), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), Span::styled(format!(" {l}  "), Style::new().fg(t.dim))]
    };
    if app.mode == Mode::Wallpaper {
        spans.push(Span::styled("imagen PNG/JPEG (vacío = quitar)  ", Style::new().fg(t.dim)));
        spans.push(Span::styled(format!("{}█", app.input), Style::new().fg(t.fg)));
    } else if app.mode == Mode::Broadcast {
        let n = app.sessions.iter().filter(|s| s.agent && s.status != Status::Exited).count();
        spans.push(Span::styled(format!("→ {n} agentes  "), Style::new().fg(t.waiting)));
        spans.push(Span::styled(format!("{}█", app.input), Style::new().fg(t.fg)));
    } else if !app.msg.is_empty() {
        spans.push(Span::styled(app.msg.clone(), Style::new().fg(t.working)));
    } else if app.mode == Mode::Prefix {
        for (k, l) in [("tecla", "acción del menú"), ("Esc", "cancelar")] {
            spans.extend(hint(k, l));
        }
    } else if app.mode == Mode::Palette {
        for (k, l) in [("↑↓", "elegir"), ("Enter", "ejecutar"), ("Esc", "cerrar"), ("◐", "los temas se previsualizan")] {
            spans.extend(hint(k, l));
        }
    } else if app.grid && app.git.is_none() {
        for (k, l) in [("←→↑↓", "mover"), ("Enter", "abrir"), ("^Space", "comandos")] {
            spans.extend(hint(k, l));
        }
    } else if app.block_sel.is_some() {
        for (k, l) in [("↑↓", "mover"), ("c", "copiar"), ("o", "salida"), ("r", "reutilizar"), ("Esc", "volver")] {
            spans.extend(hint(k, l));
        }
    } else if editor && app.git.is_none() {
        for (k, l) in [("^Space", "comandos"), ("Ctrl+↑", "bloques"), ("p", "paleta"), ("r", "git"), ("s", "panel")] {
            spans.extend(hint(k, l));
        }
    } else if app.git.is_none() {
        for (k, l) in [("^Space", "comandos"), ("p", "paleta"), ("r", "git"), ("g", "grid"), ("s", "panel"), ("q", "salir")] {
            spans.extend(hint(k, l));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::new().bg(t.bar_bg)), area);

    if !app.mouse {
        // Sin captura de ratón: recordar cómo recuperarla (estado visible, no solo un aviso).
        let tag = " ratón libre · ^Space m ";
        let w = cw(tag) as u16;
        if w < area.width / 3 {
            f.render_widget(
                Paragraph::new(tag).style(Style::new().bg(t.working).fg(t.on_accent).add_modifier(Modifier::BOLD)),
                Rect::new(area.right().saturating_sub(w + 6), area.y, w, 1),
            );
        }
    }
    if !app.sessions.is_empty() {
        let right = format!(" {}/{} ", app.focus + 1, app.sessions.len());
        let w = cw(&right) as u16;
        if w < area.width / 4 {
            f.render_widget(
                Paragraph::new(right).style(Style::new().bg(t.sel_bg).fg(t.fg)),
                Rect::new(area.right() - w, area.y, w, 1),
            );
        }
    }
}

/// Popup con lo que hace cada tecla tras el prefijo (estilo which-key).
fn which_key(f: &mut Frame, area: Rect, t: &Theme) {
    const KEYS: [(&str, &str); 14] = [
        ("p", "paleta"),
        ("n", "shell nuevo"),
        ("r", "revisar git"),
        ("g", "vista grid"),
        ("b", "broadcast"),
        ("s", "panel lateral"),
        ("j/k", "sesión sig./ant."),
        ("1-9", "ir a sesión"),
        ("u/d", "scroll historial"),
        ("w", "cerrar sesión"),
        ("q", "salir"),
        ("m", "ratón on/off"),
        ("l", "registro avisos"),
        ("␣", "enviar Ctrl+Space"),
    ];
    let (w, h) = (52u16.min(area.width), 9u16.min(area.height));
    let r = Rect::new(area.right().saturating_sub(w + 1), area.bottom().saturating_sub(h + 1), w, h);
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.working))
        .style(Style::new().bg(t.bar_bg))
        .title(Span::styled(" Ctrl+Space … ", Style::new().fg(t.working).add_modifier(Modifier::BOLD)));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let half = KEYS.len() / 2;
    let lines: Vec<Line> = (0..half)
        .map(|i| {
            let cell = |(k, l): (&str, &str)| {
                [Span::styled(format!(" {k:>3} "), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), Span::styled(format!("{l:<19}"), Style::new().fg(t.fg))]
            };
            Line::from([cell(KEYS[i]), cell(KEYS[i + half])].concat())
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

/// Avisos flotantes; devuelve sus zonas para el clic.
fn toasts(f: &mut Frame, area: Rect, app: &App, t: &Theme) -> Vec<(Rect, usize)> {
    let mut hits = vec![];
    let w = 46u16.min(area.width);
    for (i, toast) in app.toasts.iter().rev().enumerate() {
        let y = area.y + 1 + i as u16 * 3;
        if y + 3 > area.bottom() {
            break;
        }
        let r = Rect::new(area.right().saturating_sub(w + 1), y, w, 3);
        let (ic, col) = match toast.status {
            Status::Waiting => ("▲", t.waiting),
            Status::Exited => ("✕", t.exited),
            _ => ("✔", t.done),
        };
        f.render_widget(Clear, r);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(col))
            .style(Style::new().bg(t.bar_bg));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let text = truncate(&toast.text, (w as usize).saturating_sub(16));
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {ic} "), Style::new().fg(col).add_modifier(Modifier::BOLD)),
                Span::styled(text, Style::new().fg(t.fg).add_modifier(Modifier::BOLD)),
                Span::styled("  clic: ir", Style::new().fg(t.dim)),
            ])),
            inner,
        );
        hits.push((r, toast.session));
    }
    hits
}

fn palette(f: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let w = area.width.saturating_sub(4).min(78);
    let h = area.height.saturating_sub(2).min(20);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + 1, w, h);
    let items = app.palette_items();
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.accent))
        .style(Style::new().bg(t.bar_bg))
        .title(Span::styled(" ◆ paleta ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
        .title_bottom(Span::styled(
            format!(" ↑↓ elegir · Enter ejecutar · Esc salir · {} resultado{} ", items.len(), if items.len() == 1 { "" } else { "s" }),
            Style::new().fg(t.dim),
        ));
    let inner = block.inner(r);
    f.render_widget(block, r);

    let sel = app.sel.min(items.len().saturating_sub(1));
    let visible = inner.height.saturating_sub(2) as usize;
    let first = sel.saturating_sub(visible.saturating_sub(1));
    let query = if app.input.is_empty() {
        Span::styled("busca agentes, sesiones, acciones o temas…", Style::new().fg(t.dim))
    } else {
        Span::styled(app.input.clone(), Style::new().fg(t.fg))
    };
    let mut lines = vec![
        Line::from(vec![Span::styled(" ❯ ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), query, Span::styled("█", Style::new().fg(t.accent))]),
        Line::styled("─".repeat(inner.width as usize), Style::new().fg(t.sel_bg)),
    ];
    for (i, item) in items.iter().enumerate().skip(first).take(visible) {
        let selected = i == sel;
        let base = if selected { Style::new().bg(t.sel_bg) } else { Style::new() };
        let mut spans = vec![
            Span::styled(if selected { "▌" } else { " " }, base.fg(t.accent)),
            Span::styled(format!(" {} ", item.icon), base.fg(t.accent)),
        ];
        let label = truncate(&item.label, (inner.width as usize).saturating_sub(5));
        for (ci, ch) in label.chars().enumerate() {
            let style = if item.hits.contains(&ci) {
                base.fg(t.accent).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else if selected {
                base.fg(t.fg).add_modifier(Modifier::BOLD)
            } else {
                base.fg(t.fg)
            };
            spans.push(Span::styled(ch.to_string(), style));
        }
        let used: usize = spans.iter().map(|s| s.width()).sum();
        spans.push(Span::styled(" ".repeat((inner.width as usize).saturating_sub(used)), base));
        lines.push(Line::from(spans));
    }
    if items.is_empty() {
        lines.push(Line::styled("   sin resultados", Style::new().fg(t.dim)));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Registro de mensajes y avisos (los avisos flotantes caducan; aquí quedan).
fn log_view(f: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let w = area.width.saturating_sub(4).min(90);
    let h = area.height.saturating_sub(2).min(20);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + 1, w, h);
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.accent))
        .style(Style::new().bg(t.bar_bg))
        .title(Span::styled(" ☰ registro ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
        .title_bottom(Span::styled(" cualquier tecla cierra ", Style::new().fg(t.dim)));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let lines: Vec<Line> = if app.log.is_empty() {
        vec![Line::styled(" sin mensajes todavía", Style::new().fg(t.dim))]
    } else {
        app.log
            .iter()
            .rev()
            .take(inner.height as usize)
            .map(|(at, text)| {
                let ago = format!(" hace {:>6} ", elapsed(at.elapsed()));
                let room = (inner.width as usize).saturating_sub(cw(&ago) + 1);
                Line::from(vec![Span::styled(ago, Style::new().fg(t.dim)), Span::styled(truncate(text, room), Style::new().fg(t.fg))])
            })
            .collect()
    };
    f.render_widget(Paragraph::new(lines), inner);
}
