//! Rendering. Control-panel palette: red, gold, olive, cream on black.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Points};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::app::{App, Focus, LAT, LON, Node, Tab};
use crate::geo::{self, Lines};
use crate::restart::Source;
use crate::split::{self, Choice, Prompt, Split};

const BG: Color = Color::Rgb(14, 12, 10);
const PANEL: Color = Color::Rgb(22, 19, 16);
const RED: Color = Color::Rgb(204, 20, 20);
const DEEP_RED: Color = Color::Rgb(140, 14, 14);
const GOLD: Color = Color::Rgb(232, 186, 48);
const DIM_GOLD: Color = Color::Rgb(122, 98, 40);
const OLIVE: Color = Color::Rgb(112, 124, 72);
const BORDER: Color = Color::Rgb(110, 52, 40);
const GRID: Color = Color::Rgb(40, 40, 30);
const CREAM: Color = Color::Rgb(232, 222, 194);
const DIM: Color = Color::Rgb(130, 122, 104);
const GREEN: Color = Color::Rgb(70, 200, 90);
const AMBER: Color = Color::Rgb(240, 160, 30);
const BTN_GREEN: Color = Color::Rgb(28, 112, 44);
const BTN_RED: Color = Color::Rgb(158, 18, 18);
const BTN_AMBER: Color = Color::Rgb(170, 110, 0);

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(BG).fg(CREAM)), area);

    if area.width < 70 || area.height < 22 {
        let msg = Paragraph::new("TERMINAL TOO SMALL\nneed at least 70×22")
            .alignment(Alignment::Center)
            .fg(GOLD);
        f.render_widget(msg, area);
        return;
    }

    let [title, main, bottom, help] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(8),
        Constraint::Length(1),
    ])
    .areas(area);
    let [map_a, list_a] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(34)]).areas(main);
    let [status_a, button_a] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(34)]).areas(bottom);

    draw_title(f, app, title);
    draw_map(f, app, map_a);
    draw_side(f, app, list_a);
    draw_status(f, app, status_a);
    draw_button(f, app, button_a);
    draw_help(f, help);
    if app.split.prompt.is_some() {
        draw_prompt(f, &mut app.split, area);
    }
}

/// The right-hand panel, with RELAYS / SPLIT tabs in its title.
fn draw_side(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::List;
    let border = if focused { GOLD } else { DIM_GOLD };
    let split_label = match app.split.groups.len() {
        0 => " SPLIT ".to_string(),
        n => format!(" SPLIT · {n} "),
    };
    let tab = |label: &str, active: bool| {
        let style = match (active, focused) {
            (true, true) => Style::new().fg(RED).bold(),
            (true, false) => Style::new().fg(GOLD).bold(),
            (false, _) => Style::new().fg(DIM),
        };
        Span::styled(label.to_string(), style)
    };
    let edge = |s: &'static str| Span::styled(s, Style::new().fg(border));
    let title = Line::from(vec![
        edge("╡"),
        tab(" RELAYS ", app.tab == Tab::Relays),
        edge("╞═╡"),
        tab(&split_label, app.tab == Tab::Split),
        edge("╞"),
    ]);
    // Tab hit boxes: the title starts one cell in, after the corner.
    let relays_w = " RELAYS ".len() as u16 + 2;
    let split_w = split_label.chars().count() as u16 + 2;
    app.hit.side = area;
    app.hit.tabs = [
        Rect::new(area.x + 1, area.y, relays_w, 1),
        Rect::new(area.x + 1 + relays_w + 1, area.y, split_w, 1),
    ];

    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(border))
        .title(title)
        .style(Style::new().bg(PANEL));
    let inner = block.inner(area);
    f.render_widget(block, area);
    match app.tab {
        Tab::Relays => draw_list(f, app, inner),
        Tab::Split => {
            app.hit.list = Rect::default();
            draw_split(f, &mut app.split, inner, focused);
        }
    }
}

fn draw_split(f: &mut Frame, split: &mut Split, inner: Rect, focused: bool) {
    if !split::SUPPORTED {
        let text = "Split tunneling from molemap is Linux-only for now.\n\n\
                    Use the Mullvad app's settings to exclude apps on this platform.";
        f.render_widget(
            Paragraph::new(text).fg(DIM).wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    let [list_a, hint_a, add_a, launch_a, restart_a, remove_a] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    split.hit.restart = restart_a;
    split.hit.list = list_a;
    split.hit.add = add_a;
    split.hit.launch = launch_a;
    split.hit.remove = remove_a;

    if let Some(e) = &split.error {
        let text = format!("Couldn't read exclusions:\n{e}");
        f.render_widget(
            Paragraph::new(text).fg(RED).wrap(Wrap { trim: true }),
            list_a,
        );
    } else if split.groups.is_empty() {
        let text = vec![
            Line::from(Span::styled(
                "Every app goes through the VPN.",
                Style::new().fg(CREAM),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Nothing is excluded right now.",
                Style::new().fg(DIM),
            )),
        ];
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), list_a);
    } else {
        let items: Vec<ListItem> = split
            .groups
            .iter()
            .map(|g| {
                let n = g.pids.len();
                ListItem::new(Line::from(vec![
                    Span::styled("◌ ", Style::new().fg(AMBER)),
                    Span::styled(g.name.clone(), Style::new().fg(CREAM)),
                    Span::styled(
                        format!("  {n} proc{}", if n == 1 { "" } else { "s" }),
                        Style::new().fg(DIM),
                    ),
                ]))
            })
            .collect();
        let list = List::new(items).highlight_style(highlight(focused));
        f.render_stateful_widget(list, list_a, &mut split.state);
    }

    let hint = "Bypasses the VPN until it exits. Open connections keep their route until reopened.";
    f.render_widget(
        Paragraph::new(hint)
            .fg(DIM)
            .italic()
            .wrap(Wrap { trim: true }),
        hint_a,
    );

    let has_sel = split.selected().is_some();
    for (area, label, key, enabled) in [
        (add_a, "+ EXCLUDE RUNNING APP", "a", true),
        (launch_a, "▶ LAUNCH EXCLUDED", "e", true),
        (restart_a, "↻ RESTART OUTSIDE VPN", "r", has_sel),
        (remove_a, "✕ PUT BACK IN VPN", "del", has_sel),
    ] {
        let style = if enabled {
            Style::new().bg(Color::Rgb(60, 50, 26)).fg(GOLD)
        } else {
            Style::new().bg(PANEL).fg(DIM)
        };
        let pad = (area.width as usize).saturating_sub(label.chars().count() + key.len() + 3);
        let line = Line::from(vec![
            Span::styled(format!(" {label}{} ", " ".repeat(pad)), style.bold()),
            Span::styled(format!("{key} "), style),
        ]);
        f.render_widget(Paragraph::new(line).style(style), area);
    }
}

/// Centered popup box; returns the area inside its border and padding.
fn popup(f: &mut Frame, screen: Rect, title: &str, height: u16) -> (Rect, Rect) {
    let w = screen.width.saturating_sub(4).min(66);
    let h = screen.height.saturating_sub(2).min(height);
    let area = Rect::new(
        screen.x + (screen.width - w) / 2,
        screen.y + (screen.height - h) / 2,
        w,
        h,
    );
    f.render_widget(Clear, area);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(GOLD))
        .title(Line::from(vec![
            Span::styled("╡", Style::new().fg(GOLD)),
            Span::styled(format!(" {title} "), Style::new().fg(RED).bold()),
            Span::styled("╞", Style::new().fg(GOLD)),
        ]))
        .style(Style::new().bg(PANEL));
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
    (area, inner)
}

fn procs_label(n: usize) -> String {
    format!("{n} process{}", if n == 1 { "" } else { "es" })
}

/// The split-tunnel dialogs: pick a running app, launch a command, or choose
/// what to do with the app that was picked.
fn draw_prompt(f: &mut Frame, split: &mut Split, screen: Rect) {
    let excluded = split.excluded_pids();
    split.hit.prompt_list = Rect::default();
    split.hit.choices.clear();
    let Some(prompt) = split.prompt.as_mut() else {
        return;
    };

    let input_line = |input: &str| {
        Line::from(vec![
            Span::styled("› ", Style::new().fg(RED).bold()),
            Span::styled(input.to_string(), Style::new().fg(CREAM).bold()),
            Span::styled("█", Style::new().fg(GOLD)),
        ])
    };

    match prompt {
        Prompt::Launch { input } => {
            let (area, inner) = popup(f, screen, "LAUNCH OUTSIDE THE VPN", 8);
            split.hit.prompt = area;
            let [input_a, hint_a, keys_a] = Layout::vertical([
                Constraint::Length(2),
                Constraint::Fill(1),
                Constraint::Length(1),
            ])
            .areas(inner);
            f.render_widget(Paragraph::new(input_line(input)), input_a);
            let hint =
                "A command to start outside the VPN, e.g. firefox. Runs via mullvad-exclude.";
            f.render_widget(
                Paragraph::new(hint).fg(DIM).wrap(Wrap { trim: true }),
                hint_a,
            );
            f.render_widget(
                Paragraph::new("Enter launch · Esc cancel").fg(DIM_GOLD),
                keys_a,
            );
        }

        Prompt::Pick {
            input,
            matches,
            state,
        } => {
            let (area, inner) = popup(f, screen, "EXCLUDE A RUNNING APP", 18);
            split.hit.prompt = area;
            let [input_a, hint_a, body_a, keys_a] = Layout::vertical([
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Fill(1),
                Constraint::Length(1),
            ])
            .areas(inner);
            f.render_widget(Paragraph::new(input_line(input)), input_a);
            let hint = "Type part of a program name. Only your own running processes are searched.";
            f.render_widget(
                Paragraph::new(hint).fg(DIM).wrap(Wrap { trim: true }),
                hint_a,
            );
            f.render_widget(
                Paragraph::new("↑↓ choose · Enter select · Esc cancel").fg(DIM_GOLD),
                keys_a,
            );
            if input.trim().chars().count() < 2 {
                return;
            }
            if matches.is_empty() {
                f.render_widget(Paragraph::new("No matching processes.").fg(DIM), body_a);
                return;
            }
            let items: Vec<ListItem> = matches
                .iter()
                .map(|g| {
                    let mut spans = vec![
                        Span::styled(g.name.clone(), Style::new().fg(CREAM)),
                        Span::styled(
                            format!("  {}", procs_label(g.pids.len())),
                            Style::new().fg(DIM),
                        ),
                    ];
                    if g.pids.iter().all(|pid| excluded.contains(pid)) {
                        spans.push(Span::styled("  already excluded", Style::new().fg(AMBER)));
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect();
            split.hit.prompt_list = body_a;
            let list = List::new(items).highlight_style(highlight(true));
            f.render_stateful_widget(list, body_a, state);
        }

        Prompt::Choose {
            group,
            plan,
            choices,
            state,
        } => {
            let (area, inner) = popup(f, screen, &group.name.to_uppercase(), 20);
            split.hit.prompt = area;
            let name = &group.name;
            let mut lines = vec![Line::from(vec![
                Span::styled(name.clone(), Style::new().fg(CREAM).bold()),
                Span::styled(
                    format!(" · {}", procs_label(group.pids.len())),
                    Style::new().fg(DIM),
                ),
            ])];
            match plan {
                Ok(plan) => {
                    let how = match plan.launches.first().map(|l| &l.source) {
                        Some(Source::Flatpak(id)) => format!("Flatpak {id}"),
                        Some(Source::AppImage) => "AppImage".into(),
                        _ => "started from a command".into(),
                    };
                    lines.push(Line::from(Span::styled(how, Style::new().fg(DIM))));
                }
                Err(e) => lines.push(Line::from(Span::styled(
                    format!("Can't restart it: {e}."),
                    Style::new().fg(AMBER),
                ))),
            }
            lines.push(Line::from(""));
            let header_h = lines.len() as u16;
            f.render_widget(Paragraph::new(lines), inner);

            let wrap_w = inner.width.saturating_sub(4) as usize;
            let mut y = inner.y + header_h;
            for (i, choice) in choices.iter().enumerate() {
                let dim = Style::new().fg(DIM);
                let (title, body): (&str, Vec<(String, Style)>) = match choice {
                    Choice::Restart => {
                        let cmds = plan.as_ref().map_or(String::new(), |p| {
                            p.launches
                                .iter()
                                .map(|l| l.describe())
                                .collect::<Vec<_>>()
                                .join("; ")
                        });
                        (
                            "↻ RESTART OUTSIDE THE VPN",
                            vec![
                                (format!("Closes {name} and opens it again with:"), dim),
                                (cmds, Style::new().fg(CREAM)),
                                ("Everything it does bypasses the VPN.".into(), dim),
                            ],
                        )
                    }
                    Choice::ExcludeInPlace => (
                        "⇥ EXCLUDE WITHOUT RESTARTING",
                        vec![(
                            format!(
                                "New connections bypass the VPN. Ones already open \
                                 (calls, chat, downloads) stay inside it until {name} reconnects."
                            ),
                            dim,
                        )],
                    ),
                };
                let selected = state.selected() == Some(i);
                let (mark, title_style) = if selected {
                    ("▸ ", Style::new().bg(GOLD).fg(BG).bold())
                } else {
                    ("  ", Style::new().fg(GOLD).bold())
                };
                let mut text = vec![Line::from(vec![
                    Span::styled(mark, Style::new().fg(RED).bold()),
                    Span::styled(format!(" {title} "), title_style),
                ])];
                for (para, style) in body {
                    for chunk in wrap(&para, wrap_w) {
                        text.push(Line::from(Span::styled(format!("    {chunk}"), style)));
                    }
                }
                let h = (text.len() as u16).min(inner.bottom().saturating_sub(y + 1));
                let r = Rect::new(inner.x, y, inner.width, h);
                f.render_widget(Paragraph::new(text), r);
                split.hit.choices.push(r);
                y += h + 1;
            }
            let keys = if choices.is_empty() {
                "Esc close"
            } else {
                "↑↓ choose · Enter go · Esc cancel"
            };
            let keys_a = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
            f.render_widget(Paragraph::new(keys).fg(DIM_GOLD), keys_a);
        }
    }
}

/// Greedy word wrap (long words are split).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while word.chars().count() > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let head: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            lines.push(head);
        }
        let len = line.chars().count();
        if len > 0 && len + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn highlight(focused: bool) -> Style {
    if focused {
        Style::new().bg(GOLD).fg(BG).add_modifier(Modifier::BOLD)
    } else {
        Style::new().bg(Color::Rgb(60, 50, 26))
    }
}

fn panel(title: &str, focused: bool) -> Block<'_> {
    let (border, label) = if focused {
        (GOLD, RED)
    } else {
        (DIM_GOLD, DIM_GOLD)
    };
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(border))
        .title(Line::from(vec![
            Span::styled("╡", Style::new().fg(border)),
            Span::styled(format!(" {title} "), Style::new().fg(label).bold()),
            Span::styled("╞", Style::new().fg(border)),
        ]))
        .style(Style::new().bg(PANEL))
}

fn draw_title(f: &mut Frame, app: &App, area: Rect) {
    let bar = Style::new().bg(DEEP_RED).fg(GOLD);
    f.render_widget(Block::new().style(bar), area);
    let left = Line::from(vec![
        Span::styled(" ★ MOLEMAP ", Style::new().bg(GOLD).fg(DEEP_RED).bold()),
        Span::styled("  MULLVAD VPN CONTROL PANEL", bar.bold()),
    ]);
    f.render_widget(Paragraph::new(left), area);
    let cities = app.cities().count();
    let relays: usize = app
        .countries
        .iter()
        .flat_map(|c| &c.cities)
        .map(|t| t.servers.len())
        .sum();
    let right = Line::from(Span::styled(
        format!("{relays} RELAYS · {cities} CITIES "),
        bar,
    ))
    .alignment(Alignment::Right);
    f.render_widget(Paragraph::new(right), area);
}

fn draw_map(f: &mut Frame, app: &mut App, area: Rect) {
    let title = if app.view.zoom > 1.0 {
        format!("WORLD MAP · ×{:.1}", app.view.zoom)
    } else {
        "WORLD MAP".into()
    };
    let block = panel(&title, app.focus == Focus::Map);
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.hit.map = inner;
    app.view.size = (inner.width, inner.height);
    app.view.clamp();

    let view = app.view;
    let ([x0, x1], [y0, y1]) = (view.x_bounds(), view.y_bounds());
    let canvas = Canvas::default()
        .background_color(PANEL)
        .marker(Marker::Braille)
        .x_bounds([x0, x1])
        .y_bounds([y0, y1])
        .paint(|ctx| {
            // Dotted graticule, finer as we zoom in; about one dot per cell.
            let spacing = match view.zoom {
                z if z < 3.0 => 30.0,
                z if z < 8.0 => 10.0,
                _ => 5.0,
            };
            // Only over the world itself, not the margins a wide panel leaves.
            let (gx0, gx1) = (x0.max(LON[0]), x1.min(LON[1]));
            let (gy0, gy1) = (y0.max(LAT[0]), y1.min(LAT[1]));
            let step_x = (x1 - x0) / inner.width.max(1) as f64;
            let step_y = (y1 - y0) / inner.height.max(1) as f64 / 2.0;
            let mut dots = Vec::new();
            let mut lat = (gy0 / spacing).ceil() * spacing;
            while lat <= gy1 {
                let n = ((gx1 - gx0) / step_x) as usize;
                dots.extend((0..=n).map(|i| (gx0 + i as f64 * step_x, lat)));
                lat += spacing;
            }
            let mut lon = (gx0 / spacing).ceil() * spacing;
            while lon <= gx1 {
                let n = ((gy1 - gy0) / step_y) as usize;
                dots.extend((0..=n).map(|i| (lon, gy0 + i as f64 * step_y)));
                lon += spacing;
            }
            ctx.draw(&Points {
                coords: &dots,
                color: GRID,
            });
            ctx.layer();
            if view.zoom >= 2.0 {
                ctx.layer();
                ctx.draw(&Lines {
                    lines: geo::borders(),
                    color: BORDER,
                });
            }
            ctx.layer();
            ctx.draw(&Lines {
                lines: geo::coastlines(),
                color: OLIVE,
            });
        });
    f.render_widget(canvas, inner);

    let selected = app.selected();
    let sel_city = selected.and_then(Node::city);
    let sel_country = selected.map(Node::country);
    let live = app.status_city();
    let blink = (app.tick / 4).is_multiple_of(2);
    let buf = f.buffer_mut();

    // Writes a label beside (x, y) unless it would overlap one already placed.
    let mut taken: Vec<(u16, u16, u16)> = Vec::new();
    let mut label = |buf: &mut ratatui::buffer::Buffer,
                     x: u16,
                     y: u16,
                     text: &str,
                     style: Style,
                     force: bool| {
        let w = text.chars().count() as u16;
        let lx = if x + 2 + w <= inner.right() {
            x + 2
        } else {
            x.saturating_sub(w + 1).max(inner.x)
        };
        let ly = if y > inner.y {
            y - 1
        } else {
            (y + 1).min(inner.bottom().saturating_sub(1))
        };
        let clash = taken
            .iter()
            .any(|&(r, a, b)| r == ly && lx < b + 1 && a < lx + w + 1);
        if clash && !force {
            return;
        }
        taken.push((ly, lx, lx + w));
        buf.set_stringn(
            lx,
            ly,
            text,
            inner.right().saturating_sub(lx) as usize,
            style,
        );
    };

    // Plain relay dots first, highlighted ones on top.
    for (c, t) in app.cities() {
        let (lon, lat) = app.coords((c, t));
        let Some((x, y)) = app.to_cell(lon, lat) else {
            continue;
        };
        let color = if sel_city.is_none() && sel_country == Some(c) {
            GOLD
        } else {
            DIM_GOLD
        };
        buf[(x, y)].set_symbol("•").set_fg(color);
    }

    // "You are here" when the tunnel is down and we know where we are.
    if !app.status.is_up()
        && let (Some(lat), Some(lon)) = (app.status.lat, app.status.lon)
        && let Some((x, y)) = app.to_cell(lon, lat)
    {
        buf[(x, y)]
            .set_symbol("⌂")
            .set_fg(CREAM)
            .set_style(Modifier::BOLD);
    }

    let mut marks: Vec<((usize, usize), &str, Style, String)> = Vec::new();
    if let Some(city) = sel_city {
        let fg = if blink { RED } else { GOLD };
        marks.push((city, "★", Style::new().fg(fg).bold(), city_name(app, city)));
    }
    if let Some(city) = live {
        let host = app.status.hostname.clone().unwrap_or_default();
        let name = format!("{} {host}", city_name(app, city));
        marks.push((city, "◉", Style::new().fg(GREEN).bold(), name));
    }
    if let Some(city) = app.hover.filter(|h| Some(*h) != sel_city) {
        marks.push((city, "◆", Style::new().fg(CREAM), city_name(app, city)));
    }

    // Important labels claim their space first, then (when zoomed in) every
    // other visible city gets a quiet name tag wherever it fits.
    let mut placed = Vec::new();
    for (city, sym, style, text) in &marks {
        let (lon, lat) = app.coords(*city);
        let Some((x, y)) = app.to_cell(lon, lat) else {
            continue;
        };
        let fg = style.fg.unwrap_or(CREAM);
        label(
            buf,
            x,
            y,
            &format!(" {text} "),
            Style::new().bg(BG).fg(fg).bold(),
            true,
        );
        placed.push((x, y, *sym, *style));
    }
    if view.zoom >= 3.0 {
        for city in app.cities() {
            if marks.iter().any(|m| m.0 == city) {
                continue;
            }
            let (lon, lat) = app.coords(city);
            let Some((x, y)) = app.to_cell(lon, lat) else {
                continue;
            };
            let name = &app.countries[city.0].cities[city.1].name;
            label(buf, x, y, name, Style::new().fg(DIM), false);
        }
    }
    for (x, y, sym, style) in placed {
        buf[(x, y)].set_symbol(sym).set_style(style);
    }
}

fn city_name(app: &App, (c, t): (usize, usize)) -> String {
    let country = &app.countries[c];
    format!(
        "{}, {}",
        country.cities[t].name,
        country.code.to_uppercase()
    )
}

fn draw_list(f: &mut Frame, app: &mut App, inner: Rect) {
    let list_area = if app.filtering || !app.filter.is_empty() {
        let [search, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
        let cursor = if app.filtering && (app.tick / 4).is_multiple_of(2) {
            "█"
        } else {
            " "
        };
        let line = Line::from(vec![
            Span::styled(" SEARCH ", Style::new().bg(GOLD).fg(BG).bold()),
            Span::styled(format!(" {}{cursor}", app.filter), Style::new().fg(CREAM)),
        ]);
        f.render_widget(Paragraph::new(line), search);
        rest
    } else {
        inner
    };
    app.hit.list = list_area;

    let live = app.status_city();
    let live_host = app.status.hostname.as_deref();
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|&node| {
            let open = if app.is_open(node) { "▾" } else { "▸" };
            let mut spans = match node {
                Node::Country(c) => vec![
                    Span::styled(format!("{open} "), Style::new().fg(RED)),
                    Span::styled(app.label(node).to_uppercase(), Style::new().fg(GOLD).bold()),
                    Span::styled(format!(" {}", app.countries[c].code), Style::new().fg(DIM)),
                ],
                Node::City(c, t) => vec![
                    Span::styled(format!("  {open} "), Style::new().fg(DIM_GOLD)),
                    Span::styled(app.label(node).to_string(), Style::new().fg(CREAM)),
                    Span::styled(
                        format!(" ({})", app.countries[c].cities[t].servers.len()),
                        Style::new().fg(DIM),
                    ),
                ],
                Node::Server(..) => vec![
                    Span::styled("      · ", Style::new().fg(DIM_GOLD)),
                    Span::styled(app.label(node).to_string(), Style::new().fg(DIM)),
                ],
            };
            let is_live = match node {
                Node::Country(c) => live.is_some_and(|(lc, _)| lc == c),
                Node::City(c, t) => live == Some((c, t)),
                Node::Server(..) => live_host == Some(app.label(node)),
            };
            if is_live {
                spans.push(Span::styled(" ●", Style::new().fg(GREEN)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(highlight(app.focus == Focus::List))
        .scroll_padding(2);
    f.render_stateful_widget(list, list_area, &mut app.list_state);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let block = panel("STATUS", false);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let s = &app.status;
    let (state_color, state_note) = match s.state.as_str() {
        "connected" => (GREEN, "SECURE"),
        "connecting" => (AMBER, "ESTABLISHING"),
        "disconnecting" => (AMBER, "TEARING DOWN"),
        "disconnected" => (RED, "UNPROTECTED"),
        _ => (RED, "FAULT"),
    };
    let key = |k: &str| Span::styled(format!(" {k:<9}"), Style::new().fg(DIM_GOLD));
    let place = match (&s.city, &s.country) {
        (Some(city), Some(country)) => format!("{city}, {country}"),
        (None, Some(country)) => country.clone(),
        _ => "—".into(),
    };
    let target = match app.selected() {
        Some(Node::Country(c)) => format!("{} (any city)", app.countries[c].name),
        Some(Node::City(c, t)) => {
            format!(
                "{}, {}",
                app.countries[c].cities[t].name, app.countries[c].name
            )
        }
        Some(node @ Node::Server(c, ..)) => {
            format!("{} · {}", app.label(node), app.countries[c].name)
        }
        None => "—".into(),
    };

    let mut lines = vec![
        Line::from(vec![
            key("STATE"),
            Span::styled(
                format!("● {}", s.state.to_uppercase()),
                Style::new().fg(state_color).bold(),
            ),
            Span::styled(format!("  {state_note}"), Style::new().fg(state_color)),
        ]),
        Line::from(vec![
            key(if s.is_connected() {
                "RELAY"
            } else {
                "LOCATION"
            }),
            Span::raw(match (&s.hostname, s.is_connected()) {
                (Some(h), true) => format!("{h} · {place}"),
                _ => place,
            }),
        ]),
        Line::from(vec![
            key("IP"),
            Span::raw(s.ipv4.clone().unwrap_or_else(|| "—".into())),
        ]),
        Line::from(vec![
            key("TARGET"),
            Span::styled(target, Style::new().fg(GOLD)),
        ]),
    ];
    if !app.split.groups.is_empty() {
        let names: Vec<&str> = app.split.groups.iter().map(|g| g.name.as_str()).collect();
        lines.push(Line::from(vec![
            key("SPLIT"),
            Span::styled(
                format!("{} outside the VPN: {}", names.len(), names.join(", ")),
                Style::new().fg(AMBER),
            ),
        ]));
    }
    if let Some((msg, is_err)) = &app.message {
        let color = if *is_err { RED } else { AMBER };
        lines.push(Line::from(vec![
            key(if *is_err { "ERROR" } else { "ORDER" }),
            Span::styled(msg.clone(), Style::new().fg(color)),
        ]));
    }
    f.render_widget(Paragraph::new(lines).style(Style::new().fg(CREAM)), inner);
}

fn draw_button(f: &mut Frame, app: &mut App, area: Rect) {
    app.hit.button = area;
    let (bg, label, note) = if app.busy {
        (BTN_AMBER, "S T A N D   B Y", "working…")
    } else {
        match app.status.state.as_str() {
            "connected" => (BTN_RED, "D I S C O N N E C T", "tunnel active"),
            "connecting" => (BTN_AMBER, "C A N C E L", "establishing…"),
            "disconnecting" => (BTN_AMBER, "S T A N D   B Y", "working…"),
            _ => (BTN_GREEN, "C O N N E C T", "tunnel down"),
        }
    };
    let focused = app.focus == Focus::Button;
    let border = if focused { GOLD } else { CREAM };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Double
        } else {
            BorderType::Thick
        })
        .border_style(Style::new().fg(border).bg(bg))
        .style(Style::new().bg(bg));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    let text = Style::new().fg(CREAM).bg(bg);
    let lines = vec![
        Line::from(Span::styled("★", text.fg(GOLD))),
        Line::from(Span::styled(label, text.bold())),
        Line::from(Span::styled(note, text.italic())),
        Line::from(Span::styled(
            if focused {
                "▔▔▔▔▔▔▔▔▔▔"
            } else {
                ""
            },
            text.fg(GOLD),
        )),
    ];
    let top = inner.y + inner.height.saturating_sub(lines.len() as u16) / 2;
    let body = Rect {
        y: top,
        height: inner.bottom().saturating_sub(top),
        ..inner
    };
    f.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(text),
        body,
    );
}

fn draw_help(f: &mut Frame, area: Rect) {
    let keys = [
        ("TAB", "focus"),
        ("←↑↓→", "move"),
        ("ENTER", "select"),
        ("/", "search"),
        ("C", "connect"),
        ("D", "disconnect"),
        ("SPACE", "toggle"),
        ("S", "split tunnel"),
        ("WHEEL/+-", "zoom"),
        ("R-DRAG", "pan"),
        ("Q", "quit"),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (k, what) in keys {
        spans.push(Span::styled(
            format!(" {k} "),
            Style::new().bg(DIM_GOLD).fg(BG).bold(),
        ));
        spans.push(Span::styled(format!(" {what}  "), Style::new().fg(DIM)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
