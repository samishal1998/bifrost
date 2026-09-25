//! `ui::render(&App, &mut Frame)` with the brand palette (contract §10). Written by S3-L.

use crate::app::{App, Popup, VIEWS, View, c, dur, event_text, glyph, hms, state_name};
use bifrost_core::reconcile::Availability;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, HighlightSpacing, Paragraph, Row, Table, TableState, Wrap};

// ponytail: truecolor only, Terminal.app renders the RGB colours approximately; map to 256-colour indices when COLORTERM isn't truecolor
pub const NIGHT: Color = Color::Rgb(0x0B, 0x12, 0x20);
pub const FROST: Color = Color::Rgb(0xE5, 0xF0, 0xFF);
pub const TEAL: Color = Color::Rgb(0x2D, 0xD4, 0xBF);
pub const BLUE: Color = Color::Rgb(0x60, 0xA5, 0xFA);
pub const STONE: Color = Color::Rgb(0x94, 0xA3, 0xB8);
pub const AMBER: Color = Color::Rgb(0xFB, 0xBF, 0x24);
pub const ROSE: Color = Color::Rgb(0xF8, 0x71, 0x71);

const TITLES: [&str; 7] = [
    "Overview",
    "Machines",
    "Mounts",
    "Discovery",
    "Drivers",
    "Events",
    "Logs",
];

const HELP: &[(&str, &str)] = &[
    ("1-7 Tab S-Tab", "switch view"),
    ("↑/↓ j/k", "select"),
    ("m", "mount the selection (a machine means all its mounts)"),
    ("u", "unmount"),
    ("U", "force unmount: lazy detach, never kills (asks y/n)"),
    ("r", "reconcile now"),
    ("s", "discover now"),
    ("c", "reload config"),
    ("d Enter", "details"),
    ("l", "logs of the selection"),
    ("/", "filter (Enter applies, Esc clears)"),
    ("?", "this help"),
    ("q Ctrl-C", "quit"),
];

/// The colour of a state glyph or a ✓/✗ mark (cell 0 of `App::rows`).
fn mark_color(g: &str) -> Color {
    match g {
        "●" | "✓" => TEAL,
        "◌" => BLUE,
        "◐" => AMBER,
        "✕" | "✗" => ROSE,
        _ => STONE,
    }
}

pub fn render(app: &App, f: &mut Frame) {
    // NO_COLOR: every style goes through `st`, so none survives; the glyphs still carry the state
    let st = |s: Style| if app.no_color { Style::new() } else { s };
    let fg = |c: Color| st(Style::new().fg(c));
    let base = st(Style::new().fg(FROST).bg(NIGHT));
    let selected = st(Style::new().fg(NIGHT).bg(TEAL).add_modifier(Modifier::BOLD));
    let header = st(Style::new().fg(BLUE).add_modifier(Modifier::BOLD));

    f.render_widget(Block::new().style(base), f.area());
    let banner = if app.unreachable { 1 } else { 0 };
    let [tabs, banner, main, status, hints] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(banner),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    // tabs: the active one is bracketed, so it still shows without colour
    let spans = VIEWS.iter().zip(TITLES).enumerate().map(|(i, (v, t))| {
        if *v == app.view {
            Span::styled(format!("[{} {t}]", i + 1), selected)
        } else {
            Span::styled(format!(" {} {t} ", i + 1), fg(STONE))
        }
    });
    f.render_widget(Line::from_iter(spans), tabs);

    let msg = format!("bifrostd not reachable at {} — retrying", app.socket);
    let rose = st(Style::new().fg(NIGHT).bg(ROSE).add_modifier(Modifier::BOLD));
    f.render_widget(Paragraph::new(msg).style(rose), banner);

    let focus = if app.popup.is_some() { STONE } else { BLUE };
    let title = TITLES[app.view as usize];
    let block = Block::bordered()
        .border_style(fg(focus))
        .title(format!(" {title} "))
        .title_style(header);
    match (&app.status, app.view) {
        (None, _) => {
            let t = format!("connecting to bifrostd at {} …", app.socket);
            f.render_widget(Paragraph::new(t).style(fg(STONE)).block(block), main);
        }
        (Some(s), View::Overview) => {
            let h = |t: &str| Line::styled(t.to_string(), header);
            let count = |a: Availability| s.mounts.iter().filter(|m| m.state == a).count();
            let eligible = (s.machines.iter()).filter(|m| m.verdict.starts_with("allowed"));
            let mut l = vec![
                Line::styled(
                    "Bifröst",
                    st(Style::new().fg(TEAL).add_modifier(Modifier::BOLD)),
                ),
                Line::styled("Remote worlds. Local files.", fg(STONE)),
                Line::default(),
                Line::from(vec![
                    Span::raw(format!(
                        "bifrostd {}  pid {}  up {}  ",
                        c(&s.version),
                        s.pid,
                        dur(s.uptime_secs)
                    )),
                    Span::styled(c(&s.socket), fg(STONE)),
                    Span::raw(if s.ready { "" } else { "  (warming up)" }),
                ]),
                Line::raw(format!(
                    "machines {} ({} eligible)   mounts {}/{} mounted, {} degraded, {} failed",
                    s.machines.len(),
                    eligible.count(),
                    count(Availability::Mounted),
                    s.mounts.len(),
                    count(Availability::Degraded),
                    count(Availability::Failed),
                )),
                Line::default(),
                h("Providers"),
            ];
            if s.providers.is_empty() {
                l.push(Line::styled("  none (static machines only)", fg(STONE)));
            }
            for p in &s.providers {
                let (mark, text) = match (&p.last_error, p.last_ok_secs_ago) {
                    (None, None) => ("✓", format!("ok ({})", p.machines)),
                    (None, Some(t)) => ("✓", format!("ok ({}, {} ago)", p.machines, dur(t))),
                    (Some(e), None) => ("✗", format!("error: {} (never ok)", c(e))),
                    (Some(e), Some(t)) => {
                        ("✗", format!("error: {} (last ok {} ago)", c(e), dur(t)))
                    }
                };
                l.push(Line::from(vec![
                    Span::styled(format!("  {mark} "), fg(mark_color(mark))),
                    Span::raw(format!("{:<12} {text}", c(&p.name))),
                ]));
            }
            l.push(h("Config"));
            if s.config_errors.is_empty() {
                l.push(Line::from(vec![
                    Span::styled("  ✓ ", fg(TEAL)),
                    Span::raw(format!("{} (ok)", c(&s.config_path))),
                ]));
            } else {
                let t = format!(
                    "  ✗ {} (invalid; running the previous config)",
                    c(&s.config_path)
                );
                l.push(Line::styled(t, fg(ROSE)));
                for e in &s.config_errors {
                    l.push(Line::styled(format!("    {}", c(e)), fg(ROSE)));
                }
            }
            l.push(h("Recent events"));
            for r in s.events.iter().rev().take(5) {
                l.push(Line::from(vec![
                    Span::styled(format!("  {}  ", hms(r.ts_unix_ms)), fg(STONE)),
                    Span::raw(event_text(&r.event)),
                ]));
            }
            f.render_widget(Paragraph::new(l).block(block), main);
        }
        (Some(_), View::Logs) => {
            let rows = app.rows();
            let id = rows.get(app.selected).map(|r| r.0.as_str());
            let block = match id {
                Some(id) => block.title(format!(" {} (j/k: other mounts) ", c(id))),
                None => block,
            };
            let text: Vec<Line> = match (&app.log, id) {
                (_, None) => vec![Line::styled("no mounts", fg(STONE))],
                (Some(log), Some(id)) if log.mount == id => {
                    let path =
                        (!log.path.is_empty()).then(|| Line::styled(c(&log.path), fg(STONE)));
                    let mut l: Vec<Line> = path.into_iter().collect();
                    l.extend(log.lines.iter().map(|s| Line::raw(c(s))));
                    l
                }
                _ => vec![Line::styled("loading …", fg(STONE))],
            };
            // follow the tail
            let inner = main.height.saturating_sub(2) as usize;
            let scroll = text.len().saturating_sub(inner) as u16;
            f.render_widget(Paragraph::new(text).block(block).scroll((scroll, 0)), main);
        }
        (Some(_), view) => {
            // "● agent-01  tailscale  mounted  sshfs": rows() cell 0 (the mark) joins cell 1
            let head: &[&str] = match view {
                View::Machines => &["  MACHINE", "SOURCE", "STATE", "DRIVER"],
                View::Mounts => &[
                    "  MOUNT", "MACHINE", "DRIVER", "STATE", "LOCAL", "REMOTE", "ERROR",
                ],
                View::Discovery => &["  PROVIDER", "KIND", "STATUS", "MACHINES", "LAST OK"],
                View::Drivers => &["  DRIVER", "BINARY", "DETAIL", ""],
                _ => &["TIME (UTC)", "EVENT"],
            };
            let marks = view != View::Events;
            let rows: Vec<Vec<Line>> = (app.rows().into_iter())
                .map(|(_, mut cells)| {
                    if !marks {
                        return cells.into_iter().map(Line::raw).collect();
                    }
                    let mark = cells.remove(0);
                    let first = Line::from(vec![
                        Span::styled(mark.clone(), fg(mark_color(&mark))),
                        Span::raw(format!(" {}", cells.remove(0))),
                    ]);
                    [first]
                        .into_iter()
                        .chain(cells.into_iter().map(Line::raw))
                        .collect()
                })
                .collect();
            let widths = (0..head.len()).map(|i| match i + 1 == head.len() {
                true => Constraint::Fill(1),
                false => {
                    let w = rows.iter().map(|r| r[i].width()).max().unwrap_or(0);
                    Constraint::Max(w.max(head[i].chars().count()).min(48) as u16)
                }
            });
            let widths: Vec<Constraint> = widths.collect();
            let rows = rows.into_iter().map(Row::new);
            let table = Table::new(rows, widths)
                .header(Row::new(head.iter().copied()).style(header))
                .column_spacing(2)
                .row_highlight_style(selected)
                .highlight_symbol("› ")
                .highlight_spacing(HighlightSpacing::Always)
                .block(block);
            let mut state = TableState::new().with_selected(Some(app.selected));
            f.render_stateful_widget(table, main, &mut state);
        }
    }

    // the status line: the filter being typed, the active filter, the last command's result
    let line = match (&app.popup, app.filter.as_str()) {
        (Some(Popup::Filter(t)), _) => Line::styled(format!("/{t}▏"), fg(BLUE)),
        (_, "") => Line::raw(app.status_line.as_str()),
        (_, t) => Line::from(vec![
            Span::styled(format!("filter: {t} (Esc clears)  "), fg(BLUE)),
            Span::raw(app.status_line.as_str()),
        ]),
    };
    f.render_widget(line, status);
    let keys = [
        ("m", "mount"),
        ("u", "unmount"),
        ("r", "reconcile"),
        ("s", "discover"),
        ("c", "reload"),
        ("d", "details"),
        ("l", "logs"),
        ("/", "filter"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let spans = keys.iter().flat_map(|(k, v)| {
        [
            Span::styled(format!("{k} "), fg(BLUE)),
            Span::styled(format!("{v}  "), fg(STONE)),
        ]
    });
    f.render_widget(Line::from_iter(spans), hints);

    let Some(p) = &app.popup else { return };
    let (title, text, w, h): (String, Vec<Line>, _, _) = match p {
        Popup::Filter(_) => return,
        Popup::Help => {
            let l = HELP.iter().map(|(k, v)| {
                Line::from(vec![
                    Span::styled(format!("{k:<15}"), fg(BLUE)),
                    Span::raw(*v),
                ])
            });
            ("Keys".into(), l.collect(), 72, HELP.len() as u16 + 2)
        }
        Popup::ConfirmForce(id) => {
            let l = vec![
                Line::raw(format!("Force unmount {}?", c(id))),
                Line::styled("A lazy detach: no process is killed.", fg(STONE)),
                Line::default(),
                Line::from(vec![
                    Span::styled("y ", fg(BLUE)),
                    Span::styled("yes  ", fg(STONE)),
                    Span::styled("n ", fg(BLUE)),
                    Span::styled("no", fg(STONE)),
                ]),
            ];
            ("Confirm".into(), l, 50, 6)
        }
        Popup::Details(id) => {
            let l = details(app, id);
            let h = l.len() as u16 + 2;
            (c(id), l, 90, h)
        }
    };
    let area = main.centered(Constraint::Max(w), Constraint::Max(h));
    let block = Block::bordered()
        .border_style(fg(BLUE))
        .title(format!(" {title} "))
        .title_style(header);
    f.render_widget(Clear, area);
    let para = Paragraph::new(text)
        .block(block)
        .style(base)
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

/// Verdict, observation, then per mount: state, `last_error` else `detail` (B15), next retry.
fn details(app: &App, id: &str) -> Vec<Line<'static>> {
    let Some(s) = &app.status else { return vec![] };
    let kv = |k: &str, v: String| Line::raw(format!("{k:<10}{v}"));
    let mut l = vec![];
    let mut mounts: Vec<&str> = vec![id];
    if app.view == View::Machines {
        let Some(m) = s.machines.iter().find(|m| m.id == id) else {
            return vec![Line::raw("gone")];
        };
        let port = m.port.map_or(String::new(), |p| format!(":{p}"));
        let online = match m.online {
            Some(true) => "online",
            Some(false) => "offline",
            None => "online unknown",
        };
        let meta: Vec<String> = m
            .metadata
            .iter()
            .map(|(k, v)| format!("{}={}", c(k), c(v)))
            .collect();
        l.extend([
            kv("name", c(&m.name)),
            kv("verdict", c(&m.verdict)),
            kv(
                "state",
                format!("{} {}", glyph(m.state), state_name(m.state)),
            ),
            kv("source", c(&m.source)),
            kv("address", format!("{}{port}  {online}", c(&m.address))),
        ]);
        if !m.shadowed.is_empty() {
            l.push(kv("shadowed", c(&m.shadowed.join(", "))));
        }
        if !m.tags.is_empty() {
            l.push(kv("tags", c(&m.tags.join(", "))));
        }
        if !meta.is_empty() {
            l.push(kv("metadata", meta.join(" ")));
        }
        mounts = m.mounts.iter().map(String::as_str).collect();
    }
    for id in mounts {
        let Some(m) = s.mounts.iter().find(|m| m.id == id) else {
            continue;
        };
        let how = [
            m.driver.as_deref().map(c),
            m.pid.map(|p| format!("pid {p}")),
        ];
        let how = how.into_iter().flatten().collect::<Vec<_>>().join(", ");
        let how = if how.is_empty() {
            how
        } else {
            format!(" ({how})")
        };
        l.extend([
            Line::default(),
            kv("mount", c(&m.id)),
            kv(
                "state",
                format!("{} {}{how}", glyph(m.state), state_name(m.state)),
            ),
            kv("local", c(&m.local_path)),
            kv("remote", c(&m.remote)),
            kv("action", c(&m.action)),
        ]);
        if let Some(e) = m
            .last_error
            .as_deref()
            .or((!m.detail.is_empty()).then_some(&m.detail))
        {
            l.push(kv("error", c(e)));
        }
        if let Some(t) = m.retry_in_secs {
            l.push(kv("retry", format!("in {}", dur(t))));
        }
        let flags = [
            (m.held, "held"),
            (m.adopted, "adopted"),
            (!m.desired, "not desired"),
        ];
        let flags: Vec<&str> = flags.iter().filter(|f| f.0).map(|f| f.1).collect();
        l.push(kv(
            "failures",
            format!("{}  {}", m.failures, flags.join(" ")),
        ));
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::app;
    use crate::app::{Popup, View};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;

    fn draw(a: &App) -> Buffer {
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        t.draw(|f| render(a, f)).unwrap();
        t.backend().buffer().clone()
    }

    fn line(b: &Buffer, y: u16) -> String {
        (0..b.area.width).map(|x| b[(x, y)].symbol()).collect()
    }

    /// (x, y) of the first cell of `s`, searched line by line.
    fn find(b: &Buffer, s: &str) -> Option<(u16, u16)> {
        (0..b.area.height).find_map(|y| {
            let l = line(b, y);
            let i = l.find(s)?;
            Some((l[..i].chars().count() as u16, y))
        })
    }

    #[test]
    fn renders_machines_with_glyphs_and_teal() {
        let mut a = app(View::Machines);
        a.selected = 1; // build: the highlight would repaint agent-01's glyph
        let b = draw(&a);
        let (x, y) = find(&b, "agent-01").expect("agent-01 row");
        let row = line(&b, y);
        for w in ["●", "agent-01", "tailscale", "mounted", "sshfs"] {
            assert!(row.contains(w), "{w} missing from {row:?}");
        }
        assert_eq!(b[(x - 2, y)].symbol(), "●");
        assert_eq!(b[(x - 2, y)].fg, TEAL, "● mounted is Aurora Teal");
        assert_eq!(b[(x, y)].fg, FROST);
        assert_eq!(b[(x, y)].bg, NIGHT);
        // the failed machine: ✕, and the selected row is Nordic Night on Teal
        let (bx, by) = find(&b, "build").unwrap();
        assert!(line(&b, by).contains("✕"));
        assert_eq!((b[(bx, by)].fg, b[(bx, by)].bg), (NIGHT, TEAL));
        // the active tab is Teal
        let (tx, ty) = find(&b, "2 Machines").unwrap();
        assert_eq!(b[(tx, ty)].bg, TEAL);
    }

    #[test]
    fn unreachable_banner() {
        let mut a = app(View::Machines);
        a.on_status(None);
        let b = draw(&a);
        let msg = "bifrostd not reachable at /run/x.sock — retrying";
        let (x, y) = find(&b, msg).expect("banner");
        assert_eq!(b[(x, y)].bg, ROSE);
        // the last snapshot stays on screen
        assert!(find(&b, "agent-01").is_some());
    }

    #[test]
    fn no_color_disables_styles() {
        let mut a = app(View::Overview);
        a.no_color = true;
        a.on_status(None); // the banner too
        let views = [
            View::Overview,
            View::Machines,
            View::Mounts,
            View::Discovery,
            View::Drivers,
            View::Events,
            View::Logs,
        ];
        let popups = [
            None,
            Some(Popup::Help),
            Some(Popup::Details("agent-01".into())),
            Some(Popup::ConfirmForce("agent-01".into())),
            Some(Popup::Filter("ag".into())),
        ];
        for v in views {
            for p in &popups {
                a.view = v;
                a.popup = p.clone();
                let b = draw(&a);
                for c in b.content() {
                    assert_eq!(
                        (c.fg, c.bg, c.modifier),
                        (Color::Reset, Color::Reset, Modifier::empty()),
                        "{v:?} {p:?}: styled cell {c:?}"
                    );
                }
            }
        }
        a.view = View::Machines;
        a.popup = None;
        let b = draw(&a);
        assert!(find(&b, "● agent-01").is_some(), "glyphs remain");
        assert!(find(&b, "✕ build").is_some());
    }

    #[test]
    fn overview_shows_wordmark_and_tagline() {
        let b = draw(&app(View::Overview));
        let (x, y) = find(&b, "Bifröst").expect("wordmark");
        assert_eq!(b[(x, y)].fg, TEAL);
        let (x, y) = find(&b, "Remote worlds. Local files.").expect("tagline");
        assert_eq!(b[(x, y)].fg, STONE);
        // counts, provider health, last events
        assert!(find(&b, "1/2 mounted").is_some());
        assert!(find(&b, "tailscale").is_some());
        assert!(find(&b, "failed build").is_some());
    }
}
