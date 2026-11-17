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

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
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
    let frame = (app.started.elapsed().as_millis() / 100) as usize;
    f.render_widget(Block::new().style(Style::new().bg(t.bg).fg(t.fg)), f.area());

    let [side, right] = Layout::horizontal([Constraint::Length(app.sidebar_w()), Constraint::Min(1)]).areas(f.area());
    let [main, bar] = Layout::vertical([Constraint::Min(2), Constraint::Length(1)]).areas(right);
    let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(main);
    // Tamaño estable para los agentes, sin importar qué vista esté encima.
    app.resize(body.height, body.width);

    app.hit_new = if side.width > 0 { sidebar(f, side, app, &t, frame) } else { Rect::default() };
    if let Some(g) = app.git.as_mut() {
        g.draw(f, main, &t);
    } else if app.grid {
        grid(f, main, app, &t, frame);
    } else if let Some(s) = app.sessions.get(app.focus) {
        header(f, head, s, &t, frame);
        let scr = s.parser.screen();
        draw_screen(f.buffer_mut(), body, scr, 0, &t);
        if s.scroll == 0 && !scr.hide_cursor() && app.mode == Mode::Normal {
            let (r, c) = scr.cursor_position();
            f.set_cursor_position(Position::new(body.x + c, body.y + r));
        }
        if s.scroll > 0 {
            let tag = format!(" ↑ historial {} · cualquier tecla vuelve ", s.scroll);
            let w = (tag.chars().count() as u16).min(body.width);
            f.render_widget(
                Paragraph::new(tag).style(Style::new().bg(t.accent).fg(t.on_accent).add_modifier(Modifier::BOLD)),
                Rect::new(body.right() - w, body.y, w, 1),
            );
        }
    } else {
        welcome(f, main, &t);
    }
    status_bar(f, bar, app, &t);
    match app.mode {
        Mode::Palette => palette(f, main, app, &t),
        Mode::Prefix => which_key(f, main, &t),
        _ => {}
    }
    app.hit_toasts = toasts(f, main, app, &t);
}

/// Cabecera de la sesión enfocada: estado, nombre, tarea, rama, carpeta y tiempo en ese estado.
fn header(f: &mut Frame, area: Rect, s: &Session, t: &Theme, frame: usize) {
    let (ic, col) = icon(s, t, frame);
    let right = format!(
        "{}{}  ·  {} ",
        s.branch.as_ref().map_or(String::new(), |b| format!("⎇ {b}  ·  ")),
        short_path(&s.cwd),
        elapsed(s.since.elapsed())
    );
    let rw = (right.chars().count() as u16).min(area.width / 2);
    let mut left = vec![
        Span::styled(format!(" {ic} {} ", s.status.label()), Style::new().bg(col).fg(t.on_accent).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {} ", s.name), Style::new().fg(t.fg).add_modifier(Modifier::BOLD)),
    ];
    if s.agent && !s.title().is_empty() {
        left.push(Span::styled(format!(" {}", s.title()), Style::new().fg(t.dim)));
    }
    f.render_widget(Paragraph::new(Line::from(left)).style(Style::new().bg(t.bar_bg)), area);
    f.render_widget(
        Paragraph::new(truncate(&right, rw as usize)).style(Style::new().bg(t.bar_bg).fg(t.dim)),
        Rect::new(area.right() - rw, area.y, rw, 1),
    );
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
    let (mode, col) = match app.mode {
        Mode::Prefix => ("PREFIJO", t.working),
        Mode::Palette => ("PALETA", t.accent),
        Mode::Broadcast => ("BROADCAST", t.waiting),
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
    if app.mode == Mode::Broadcast {
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
    } else if app.git.is_none() {
        for (k, l) in [("^Space", "comandos"), ("p", "paleta"), ("r", "git"), ("g", "grid"), ("s", "panel"), ("q", "salir")] {
            spans.extend(hint(k, l));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::new().bg(t.bar_bg)), area);

    if !app.sessions.is_empty() {
        let right = format!(" {}/{} ", app.focus + 1, app.sessions.len());
        let w = right.chars().count() as u16;
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
    const KEYS: [(&str, &str); 12] = [
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
        ("␣", "enviar Ctrl+Space"),
    ];
    let (w, h) = (52u16.min(area.width), 8u16.min(area.height));
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
