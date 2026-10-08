mod app;
mod geo;
mod mullvad;
mod restart;
mod split;
mod ui;

use std::io::stdout;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use ratatui::crossterm::execute;

use app::{App, Msg, Tab};

/// Poll interval; also drives blinking.
const TICK: Duration = Duration::from_millis(100);
/// Fallback status poll, in ticks, in case `status listen` misses something.
const POLL_TICKS: u64 = 50;
/// Split-tunnel poll while that tab is showing, in ticks.
const SPLIT_POLL_TICKS: u64 = 20;

const HELP: &str = "\
molemap — a terminal UI for Mullvad VPN

Usage: molemap [-h | --help] [-V | --version]

Requires the Mullvad VPN app, with its `mullvad` CLI on PATH.

Keys:   Tab focus · arrows/hjkl move · Enter select · / search
        c connect · d disconnect · Space toggle · +/- zoom · 0 reset · q quit
        s split-tunnel tab · a exclude a running app · e launch an app excluded
Mouse:  click select · double-click connect · wheel zoom · right-drag pan";

fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("-h" | "--help") => {
            println!("{HELP}");
            return Ok(());
        }
        Some("-V" | "--version") => {
            println!("molemap {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(other) => anyhow::bail!("unknown argument '{other}' (try --help)"),
    }

    let countries = mullvad::relay_list()?;
    let status = mullvad::status()?;

    let (tx, rx) = mpsc::channel();
    mullvad::listen(tx.clone(), || Msg::Refresh);
    let mut app = App::new(countries, status, tx.clone());

    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture)?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        hook(info);
    }));

    let mut refreshing = false;
    let result = (|| -> Result<()> {
        while !app.quit {
            terminal.draw(|f| ui::draw(f, &mut app))?;

            if event::poll(TICK)? {
                match event::read()? {
                    Event::Key(key) => app.on_key(key),
                    Event::Mouse(mouse) => app.on_mouse(mouse),
                    _ => {}
                }
            }

            app.tick += 1;
            if app.tick.is_multiple_of(POLL_TICKS) {
                let _ = tx.send(Msg::Refresh);
            }
            // Excluded processes come and go; poll faster while they're on screen.
            let split_every = if app.tab == Tab::Split {
                SPLIT_POLL_TICKS
            } else {
                POLL_TICKS
            };
            if app.tick.is_multiple_of(split_every) {
                app.split.refresh();
            }
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    Msg::Refresh if !refreshing => {
                        refreshing = true;
                        let tx = tx.clone();
                        thread::spawn(move || {
                            if let Ok(s) = mullvad::status() {
                                let _ = tx.send(Msg::Status(s));
                            } else {
                                let _ = tx.send(Msg::Status(Default::default()));
                            }
                        });
                    }
                    Msg::Refresh => {}
                    Msg::Status(s) => {
                        refreshing = false;
                        if !s.state.is_empty() {
                            app.set_status(s);
                        }
                    }
                    Msg::Done(r) => app.on_done(r),
                    Msg::RefreshSplit => app.split.refresh(),
                    Msg::SplitList(list) => app.split.set_list(list),
                    Msg::SplitChanged(pids, excluded) => app.split.changed(pids, excluded),
                    Msg::Note(r) => {
                        app.message = Some(match r {
                            Ok(note) => (note, false),
                            Err(e) => (e, true),
                        })
                    }
                }
            }
        }
        Ok(())
    })();

    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}
