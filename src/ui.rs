use crate::session::{Session, Status};
use crate::config::Theme;
use crate::{App, Mode};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use ratatui::Frame;

pub const SIDEBAR: u16 = 28;

pub fn grid_cols(n: usize) -> usize {
    (n as f64).sqrt().ceil().max(1.0) as usize
}

fn icon(s: &Session, t: &Theme) -> (&'static str, Color) {
    match s.status {
        Status::Working => ("●", t.working),
        Status::Waiting => ("▲", t.waiting),
        Status::Idle if s.attention => ("✔", t.done),
        Status::Idle => ("○", t.dim),
        Status::Exited => ("✕", t.exited),
    }
}

fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Copia la pantalla emulada al buffer de ratatui, empezando en la fila `top` de la pantalla.
fn draw_screen(buf: &mut Buffer, area: Rect, screen: &vt100::Screen, top: u16) {
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
                    out.set_style(Style::new().fg(color(c.fgcolor())).bg(color(c.bgcolor())).add_modifier(m));
                }
                None => {
                    out.reset();
                }
            }
        }
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = app.theme;
    let [side, right] = Layout::horizontal([Constraint::Length(SIDEBAR), Constraint::Min(1)]).areas(f.area());
    let [main, bar] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(right);
    app.resize(main.height, main.width);

    sidebar(f, side, app);
    if let Some(g) = app.git.as_mut() {
        g.draw(f, main, &t);
    } else if app.grid {
        grid(f, main, app);
    } else if let Some(s) = app.sessions.get(app.focus) {
        let scr = s.parser.screen();
        draw_screen(f.buffer_mut(), main, scr, 0);
        if s.scroll == 0 && !scr.hide_cursor() && app.mode == Mode::Normal {
            let (r, c) = scr.cursor_position();
            f.set_cursor_position(Position::new(main.x + c, main.y + r));
        }
        if s.scroll > 0 {
            let tag = format!(" ↑ historial {} ", s.scroll);
            let w = tag.chars().count() as u16;
            f.render_widget(
                Paragraph::new(tag).style(Style::new().bg(t.accent).fg(t.on_accent)),
                Rect::new(main.right().saturating_sub(w), main.y, w.min(main.width), 1),
            );
        }
    } else {
        f.render_widget(
            Paragraph::new("\n  Sin sesiones.\n\n  Ctrl+Space n  → shell\n  Ctrl+Space p  → paleta (agentes, worktrees...)")
                .style(Style::new().fg(t.dim)),
            main,
        );
    }
    status_bar(f, bar, app);
    if app.mode == Mode::Palette {
        palette(f, main, app);
    }
}

fn sidebar(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let count = |st: Status| app.sessions.iter().filter(|s| s.status == st).count();
    let summary = format!(
        " ●{} ▲{} ✔{} ",
        count(Status::Working),
        count(Status::Waiting),
        app.sessions.iter().filter(|s| s.attention).count()
    );
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.dim))
        .title(Span::styled(" kp ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
        .title_bottom(Span::styled(summary, Style::new().fg(t.dim)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let w = inner.width as usize;
    let mut lines = vec![];
    for (i, s) in app.sessions.iter().enumerate() {
        let (ic, col) = icon(s, &t);
        let sel = i == app.focus;
        let bg = if sel { Style::new().bg(t.sel_bg) } else { Style::new() };
        let mut name = Style::new().add_modifier(Modifier::BOLD);
        if s.attention {
            name = name.fg(t.done);
        }
        // Los agentes ponen en el título la tarea actual; el de un shell suele ser la ruta del .exe.
        let sub = if !s.agent || s.title().is_empty() {
            s.cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        } else {
            s.title().to_string()
        };
        lines.push(
            Line::from(vec![
                Span::styled(if sel { "▌" } else { " " }, Style::new().fg(t.accent)),
                Span::styled(ic, Style::new().fg(col)),
                Span::styled(format!(" {} ", i + 1), Style::new().fg(t.dim)),
                Span::styled(s.name.clone(), name),
                Span::styled(format!(" {}", s.status.label()), Style::new().fg(t.dim)),
            ])
            .style(bg),
        );
        lines.push(Line::styled(format!("    {}", truncate(&sub, w.saturating_sub(4))), bg.fg(t.dim)));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
}

fn grid(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let n = app.sessions.len();
    if n == 0 {
        return;
    }
    let cols = grid_cols(n);
    let rows = n.div_ceil(cols);
    let (w, h) = (area.width / cols as u16, area.height / rows as u16);
    for (i, s) in app.sessions.iter().enumerate() {
        let tile = Rect::new(area.x + (i % cols) as u16 * w, area.y + (i / cols) as u16 * h, w, h);
        let (ic, col) = icon(s, &t);
        let focused = i == app.focus;
        let block = Block::bordered()
            .border_type(if focused { BorderType::Thick } else { BorderType::Rounded })
            .border_style(Style::new().fg(if focused { t.accent } else { col }))
            .title(Line::from(vec![
                Span::styled(format!(" {ic} "), Style::new().fg(col)),
                Span::styled(format!("{} {} ", i + 1, s.name), Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(format!("{} ", s.status.label()), Style::new().fg(t.dim)),
            ]));
        let inner = block.inner(tile);
        f.render_widget(block, tile);
        // Muestra las últimas filas con contenido, sin redimensionar al agente.
        let scr = s.parser.screen();
        let last = scr.rows(0, scr.size().1.get()).collect::<Vec<_>>().iter().rposition(|r| !r.trim().is_empty());
        let top = last.map_or(0, |l| (l as u16 + 1).saturating_sub(inner.height));
        draw_screen(f.buffer_mut(), inner, scr, top);
    }
}

fn status_bar(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let key = Style::new().fg(t.on_accent).bg(t.accent).add_modifier(Modifier::BOLD);
    let left: Line = match app.mode {
        Mode::Prefix => Line::from(vec![
            Span::styled(" PREFIJO ", key),
            Span::raw(" n shell · p paleta · r revisar git · g grid · b broadcast · j/k · 1-9 · u/d scroll · w cerrar · q salir"),
        ]),
        Mode::Broadcast => {
            let n = app.sessions.iter().filter(|s| s.agent && s.status != Status::Exited).count();
            Line::from(vec![
                Span::styled(format!(" BROADCAST → {n} agentes "), key.bg(t.waiting)),
                Span::raw(format!(" {}█", app.input)),
            ])
        }
        _ if !app.msg.is_empty() => Line::styled(format!(" {}", app.msg), Style::new().fg(t.working)),
        _ if app.grid => Line::from(vec![
            Span::styled(" GRID ", key),
            Span::styled(" flechas/hjkl mover · Enter abrir · Ctrl+Space comandos", Style::new().fg(t.dim)),
        ]),
        _ => Line::from(vec![
            Span::styled(" ^Space ", key),
            Span::styled(" n shell · p paleta · r revisar git · g grid · b broadcast · q salir", Style::new().fg(t.dim)),
        ]),
    };
    f.render_widget(Paragraph::new(left).style(Style::new().bg(t.bar_bg)), area);
    if let Some(s) = app.sessions.get(app.focus) {
        let right = format!(" {} · {} · {} ", s.name, s.status.label(), s.cwd.display());
        let w = (right.chars().count() as u16).min(area.width / 3);
        let r = Rect::new(area.right() - w, area.y, w, 1);
        f.render_widget(Paragraph::new(truncate(&right, w as usize)).style(Style::new().fg(t.dim)), r);
    }
}

fn palette(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let w = area.width.saturating_sub(4).min(72);
    let h = area.height.saturating_sub(2).min(18);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + 1, w, h);
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.accent))
        .title(Span::styled(" paleta ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
        .title_bottom(Span::styled(" ↑↓ elegir · Enter ejecutar · Esc salir ", Style::new().fg(t.dim)));
    let inner = block.inner(r);
    f.render_widget(block, r);

    let items = app.palette_items();
    let sel = app.sel.min(items.len().saturating_sub(1));
    let visible = inner.height.saturating_sub(2) as usize;
    let first = sel.saturating_sub(visible.saturating_sub(1));
    let mut lines = vec![
        Line::from(vec![Span::styled("❯ ", Style::new().fg(t.accent)), Span::raw(app.input.clone()), Span::raw("█")]),
        Line::raw(""),
    ];
    for (i, (label, _)) in items.iter().enumerate().skip(first).take(visible) {
        let style = if i == sel { Style::new().bg(t.sel_bg).fg(t.accent).add_modifier(Modifier::BOLD) } else { Style::new() };
        lines.push(Line::styled(format!(" {}", truncate(label, inner.width as usize - 2)), style));
    }
    if items.is_empty() {
        lines.push(Line::styled(" (sin resultados)", Style::new().fg(t.dim)));
    }
    f.render_widget(Paragraph::new(lines), inner);
}
