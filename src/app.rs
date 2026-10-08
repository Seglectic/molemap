//! Application state and input handling.

use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;

use crate::mullvad::{self, Country, Status};
use crate::split::{PromptKind, Split};

/// Whole-world map window in degrees (Antarctica cropped).
pub const LON: [f64; 2] = [-180.0, 180.0];
pub const LAT: [f64; 2] = [-58.0, 84.0];

const MAX_ZOOM: f64 = 32.0;
const ZOOM_STEP: f64 = 1.25;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Terminal cells are roughly twice as tall as they are wide.
const CELL_ASPECT: f64 = 2.0;

/// The part of the world the map shows: a center and a zoom factor, fitted to
/// the map panel's size so the world keeps its proportions however the
/// terminal is shaped.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub lon: f64,
    pub lat: f64,
    pub zoom: f64,
    /// Map panel size in cells, updated every draw.
    pub size: (u16, u16),
}

impl View {
    pub fn world() -> Self {
        View {
            lon: (LON[0] + LON[1]) / 2.0,
            lat: (LAT[0] + LAT[1]) / 2.0,
            zoom: 1.0,
            size: (1, 1),
        }
    }

    /// Back to the whole world, keeping the panel size.
    pub fn reset(&mut self) {
        *self = View {
            size: self.size,
            ..View::world()
        };
    }

    /// Half the visible span in degrees (lon, lat). At zoom 1 the whole world
    /// fits; the other axis gets whatever the panel's shape leaves over.
    fn half(&self) -> (f64, f64) {
        let (w, h) = (self.size.0.max(1) as f64, self.size.1.max(1) as f64);
        let deg_per_col =
            ((LON[1] - LON[0]) / w).max((LAT[1] - LAT[0]) / (h * CELL_ASPECT)) / self.zoom;
        (deg_per_col * w / 2.0, deg_per_col * h * CELL_ASPECT / 2.0)
    }

    pub fn x_bounds(&self) -> [f64; 2] {
        let (hx, _) = self.half();
        [self.lon - hx, self.lon + hx]
    }

    pub fn y_bounds(&self) -> [f64; 2] {
        let (_, hy) = self.half();
        [self.lat - hy, self.lat + hy]
    }

    pub fn contains(&self, lon: f64, lat: f64) -> bool {
        let ([x0, x1], [y0, y1]) = (self.x_bounds(), self.y_bounds());
        (x0..=x1).contains(&lon) && (y0..=y1).contains(&lat)
    }

    /// Keep the zoom in range and the window over the world. An axis wider
    /// than the world stays centered on it.
    pub fn clamp(&mut self) {
        self.zoom = self.zoom.clamp(1.0, MAX_ZOOM);
        let (hx, hy) = self.half();
        let fit = |c: f64, half: f64, [lo, hi]: [f64; 2]| {
            if half * 2.0 >= hi - lo {
                (lo + hi) / 2.0
            } else {
                c.clamp(lo + half, hi - half)
            }
        };
        self.lon = fit(self.lon, hx, LON);
        self.lat = fit(self.lat, hy, LAT);
    }
}

/// What a left click landed on, for double-click detection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Click {
    City((usize, usize)),
    Row(Node),
}

pub enum Msg {
    /// Tunnel state may have changed; re-read status.
    Refresh,
    Status(Status),
    /// A connect/disconnect command finished.
    Done(Result<(), String>),
    /// Re-read the split-tunnel exclusions.
    RefreshSplit,
    /// Excluded PIDs as the daemon reports them.
    SplitList(Result<Vec<u32>, String>),
    /// We excluded (true) or un-excluded (false) these PIDs.
    SplitChanged(Vec<u32>, bool),
    /// Something finished that's worth a line in the status panel.
    Note(Result<String, String>),
}

/// What the right-hand panel shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Relays,
    Split,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Map,
    List,
    Button,
}

/// A row in the location tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Node {
    Country(usize),
    City(usize, usize),
    Server(usize, usize, usize),
}

impl Node {
    pub fn country(self) -> usize {
        match self {
            Node::Country(c) | Node::City(c, _) | Node::Server(c, _, _) => c,
        }
    }

    pub fn city(self) -> Option<(usize, usize)> {
        match self {
            Node::Country(_) => None,
            Node::City(c, t) | Node::Server(c, t, _) => Some((c, t)),
        }
    }
}

/// Screen regions from the last draw, used for mouse hit-testing.
#[derive(Default)]
pub struct Hit {
    pub map: Rect,
    pub list: Rect,
    pub button: Rect,
    /// The whole right-hand panel, and its RELAYS / SPLIT tab labels.
    pub side: Rect,
    pub tabs: [Rect; 2],
}

pub struct App {
    pub countries: Vec<Country>,
    pub status: Status,
    pub focus: Focus,
    pub expanded: HashSet<Node>,
    pub rows: Vec<Node>,
    pub list_state: ListState,
    pub filter: String,
    pub filtering: bool,
    pub hover: Option<(usize, usize)>,
    /// A command is in flight; the button shows a working state.
    pub busy: bool,
    pub message: Option<(String, bool)>,
    pub tick: u64,
    pub hit: Hit,
    pub tab: Tab,
    pub split: Split,
    pub view: View,
    /// Right/middle-drag pan: where the drag started and the view at that moment.
    drag: Option<(u16, u16, View)>,
    last_click: Option<(Instant, Click)>,
    /// The previous list click folded/unfolded its row.
    click_toggled: bool,
    pub quit: bool,
    tx: Sender<Msg>,
}

impl App {
    pub fn new(countries: Vec<Country>, status: Status, tx: Sender<Msg>) -> Self {
        let mut app = App {
            countries,
            status,
            focus: Focus::Map,
            expanded: HashSet::new(),
            rows: Vec::new(),
            list_state: ListState::default(),
            filter: String::new(),
            filtering: false,
            hover: None,
            busy: false,
            message: None,
            tick: 0,
            hit: Hit::default(),
            tab: Tab::Relays,
            split: Split::new(tx.clone()),
            view: View::world(),
            drag: None,
            last_click: None,
            click_toggled: false,
            quit: false,
            tx,
        };
        app.rebuild_rows();
        app.split.refresh();
        match app.status_city() {
            Some((c, t)) => app.select(Node::City(c, t)),
            None => app.list_state.select(Some(0)),
        }
        app
    }

    // ---------------------------------------------------------------- tree

    pub fn label(&self, node: Node) -> &str {
        match node {
            Node::Country(c) => &self.countries[c].name,
            Node::City(c, t) => &self.countries[c].cities[t].name,
            Node::Server(c, t, s) => &self.countries[c].cities[t].servers[s],
        }
    }

    fn children(&self, node: Node) -> Vec<Node> {
        match node {
            Node::Country(c) => (0..self.countries[c].cities.len())
                .map(|t| Node::City(c, t))
                .collect(),
            Node::City(c, t) => (0..self.countries[c].cities[t].servers.len())
                .map(|s| Node::Server(c, t, s))
                .collect(),
            Node::Server(..) => Vec::new(),
        }
    }

    pub fn has_children(&self, node: Node) -> bool {
        !matches!(node, Node::Server(..))
    }

    pub fn is_open(&self, node: Node) -> bool {
        !self.filter.is_empty() || self.expanded.contains(&node)
    }

    fn matches(&self, node: Node) -> bool {
        let f = self.filter.to_lowercase();
        let hit = |s: &str| s.to_lowercase().contains(&f);
        match node {
            Node::Country(c) => hit(&self.countries[c].name) || hit(&self.countries[c].code),
            Node::City(c, t) => hit(&self.countries[c].cities[t].name),
            Node::Server(c, t, s) => hit(&self.countries[c].cities[t].servers[s]),
        }
    }

    /// With a filter, a row is shown when it, an ancestor, or a descendant matches.
    fn visible(&self, node: Node, ancestor_hit: bool) -> bool {
        self.filter.is_empty()
            || ancestor_hit
            || self.matches(node)
            || self
                .children(node)
                .into_iter()
                .any(|n| self.visible(n, false))
    }

    pub fn rebuild_rows(&mut self) {
        let selected = self.selected();
        let mut rows = Vec::new();
        for c in 0..self.countries.len() {
            let country = Node::Country(c);
            if !self.visible(country, false) {
                continue;
            }
            rows.push(country);
            if !self.is_open(country) {
                continue;
            }
            let c_hit = !self.filter.is_empty() && self.matches(country);
            for city in self.children(country) {
                if !self.visible(city, c_hit) {
                    continue;
                }
                rows.push(city);
                if !self.is_open(city) {
                    continue;
                }
                let t_hit = c_hit || (!self.filter.is_empty() && self.matches(city));
                rows.extend(
                    self.children(city)
                        .into_iter()
                        .filter(|s| self.visible(*s, t_hit)),
                );
            }
        }
        self.rows = rows;
        let idx = selected
            .and_then(|n| self.rows.iter().position(|r| *r == n))
            .or((!self.rows.is_empty()).then_some(0));
        self.list_state.select(idx);
    }

    pub fn selected(&self) -> Option<Node> {
        self.list_state
            .selected()
            .and_then(|i| self.rows.get(i).copied())
    }

    pub fn selected_city(&self) -> Option<(usize, usize)> {
        self.selected().and_then(Node::city)
    }

    /// Select a node, expanding its parents so it is visible.
    pub fn select(&mut self, node: Node) {
        if let Some((c, t)) = node.city() {
            self.expanded.insert(Node::Country(c));
            if matches!(node, Node::Server(..)) {
                self.expanded.insert(Node::City(c, t));
            }
        }
        if !self.filter.is_empty() && !self.visible(node, false) {
            self.filter.clear();
        }
        self.rebuild_rows();
        if let Some(i) = self.rows.iter().position(|r| *r == node) {
            self.list_state.select(Some(i));
        }
    }

    /// Map picks only keep the picked country unfolded in the list.
    fn select_from_map(&mut self, (c, t): (usize, usize)) {
        self.expanded.clear();
        self.select(Node::City(c, t));
    }

    fn toggle_open(&mut self, node: Node) {
        if !self.expanded.remove(&node) {
            self.expanded.insert(node);
        }
        self.rebuild_rows();
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0) as isize + delta;
        self.list_state
            .select(Some(i.clamp(0, self.rows.len() as isize - 1) as usize));
    }

    // ---------------------------------------------------------------- map

    pub fn cities(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.countries
            .iter()
            .enumerate()
            .flat_map(|(c, country)| (0..country.cities.len()).map(move |t| (c, t)))
    }

    pub fn coords(&self, (c, t): (usize, usize)) -> (f64, f64) {
        let city = &self.countries[c].cities[t];
        (city.lon, city.lat)
    }

    /// The relay city we're currently connected through, if any.
    pub fn status_city(&self) -> Option<(usize, usize)> {
        if !self.status.is_up() {
            return None;
        }
        if let Some(host) = &self.status.hostname {
            for (c, t) in self.cities() {
                if self.countries[c].cities[t]
                    .servers
                    .iter()
                    .any(|s| s == host)
                {
                    return Some((c, t));
                }
            }
        }
        let name = self.status.city.as_deref()?;
        self.cities()
            .find(|&(c, t)| self.countries[c].cities[t].name == name)
    }

    /// Terminal cell (col, row) for a lon/lat, matching how ratatui's canvas
    /// places its points. `None` when it's outside the current view.
    pub fn to_cell(&self, lon: f64, lat: f64) -> Option<(u16, u16)> {
        if !self.view.contains(lon, lat) {
            return None;
        }
        let (a, [x0, x1], [y0, y1]) = (self.hit.map, self.view.x_bounds(), self.view.y_bounds());
        let x = (lon - x0) / (x1 - x0) * a.width.saturating_sub(1) as f64;
        let y = (y1 - lat) / (y1 - y0) * a.height.saturating_sub(1) as f64;
        Some((a.x + x as u16, a.y + y as u16))
    }

    /// Degrees per terminal cell (horizontal, vertical).
    fn cell_size(&self) -> (f64, f64) {
        let (a, [x0, x1], [y0, y1]) = (self.hit.map, self.view.x_bounds(), self.view.y_bounds());
        let w = a.width.saturating_sub(1).max(1) as f64;
        let h = a.height.saturating_sub(1).max(1) as f64;
        ((x1 - x0) / w, (y1 - y0) / h)
    }

    /// Lon/lat under a terminal cell.
    fn to_geo(&self, col: u16, row: u16) -> (f64, f64) {
        let (dx, dy) = self.cell_size();
        let (a, [x0, _], [_, y1]) = (self.hit.map, self.view.x_bounds(), self.view.y_bounds());
        (x0 + (col - a.x) as f64 * dx, y1 - (row - a.y) as f64 * dy)
    }

    /// Zoom by `factor`, keeping the point under (col, row) fixed on screen.
    fn zoom_at(&mut self, col: u16, row: u16, factor: f64) {
        let (lon, lat) = self.to_geo(col, row);
        let old = self.view.zoom;
        self.view.zoom = (old * factor).clamp(1.0, MAX_ZOOM);
        let k = old / self.view.zoom;
        self.view.lon = lon - (lon - self.view.lon) * k;
        self.view.lat = lat - (lat - self.view.lat) * k;
        self.view.clamp();
    }

    /// Zoom around the middle of the map.
    fn zoom_center(&mut self, factor: f64) {
        let a = self.hit.map;
        self.zoom_at(a.x + a.width / 2, a.y + a.height / 2, factor);
    }

    /// Recenter on a city if it has scrolled out of view.
    fn reveal(&mut self, (c, t): (usize, usize)) {
        let (lon, lat) = self.coords((c, t));
        if !self.view.contains(lon, lat) {
            self.view.lon = lon;
            self.view.lat = lat;
            self.view.clamp();
        }
    }

    /// Left click: true if it repeats the previous click on the same target.
    fn is_double(&mut self, target: Click) -> bool {
        let now = Instant::now();
        let double = self
            .last_click
            .is_some_and(|(t, prev)| prev == target && now - t < DOUBLE_CLICK);
        self.last_click = (!double).then_some((now, target));
        double
    }

    /// Nearest city to a terminal cell, within a few cells.
    pub fn city_at(&self, col: u16, row: u16) -> Option<(usize, usize)> {
        let mut best = None;
        let mut best_d = 9.0; // 3 cells squared
        for city in self.cities() {
            let (lon, lat) = self.coords(city);
            let Some((x, y)) = self.to_cell(lon, lat) else {
                continue;
            };
            // Cells are about twice as tall as they are wide.
            let d = (x as f64 - col as f64).powi(2) + (2.0 * (y as f64 - row as f64)).powi(2);
            if d < best_d {
                best_d = d;
                best = Some(city);
            }
        }
        best
    }

    /// Jump to the nearest city in a direction (dx, dy in lon/lat sign).
    fn map_step(&mut self, dx: f64, dy: f64) {
        let Some(from) = self.selected_city().or_else(|| self.cities().next()) else {
            return;
        };
        let (x0, y0) = self.coords(from);
        let mut best = None;
        let mut best_score = f64::MAX;
        for city in self.cities() {
            if city == from {
                continue;
            }
            let (x, y) = self.coords(city);
            let (ex, ey) = (x - x0, y - y0);
            let along = ex * dx + ey * dy;
            if along <= 0.0 {
                continue;
            }
            let across = (ex * dy - ey * dx).abs();
            let score = along + 2.5 * across;
            if score < best_score {
                best_score = score;
                best = Some(city);
            }
        }
        if let Some(city) = best {
            self.select_from_map(city);
            self.reveal(city);
        }
    }

    // ---------------------------------------------------------------- actions

    pub fn set_status(&mut self, status: Status) {
        self.status = status;
    }

    /// Big button: disconnect when up, otherwise connect to the selection.
    pub fn press_button(&mut self) {
        if self.status.is_up() {
            self.disconnect();
        } else {
            self.connect();
        }
    }

    pub fn connect(&mut self) {
        let target = self.selected().map(|node| {
            let c = &self.countries[node.country()];
            let city = node.city().map(|(_, t)| c.cities[t].code.clone());
            let host = match node {
                Node::Server(_, t, s) => Some(c.cities[t].servers[s].clone()),
                _ => None,
            };
            (c.code.clone(), city, host, self.label(node).to_string())
        });
        let what = target
            .as_ref()
            .map_or("current relay".into(), |t| t.3.clone());
        self.spawn(format!("CONNECTING → {what}"), move || match &target {
            Some((cc, city, host, _)) => mullvad::connect_to(cc, city.as_deref(), host.as_deref()),
            None => mullvad::connect(),
        });
    }

    pub fn disconnect(&mut self) {
        self.spawn("DISCONNECTING".into(), mullvad::disconnect);
    }

    fn spawn(&mut self, note: String, f: impl FnOnce() -> anyhow::Result<()> + Send + 'static) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.message = Some((note, false));
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::Done(f().map_err(|e| e.to_string())));
            let _ = tx.send(Msg::Refresh);
        });
    }

    pub fn on_done(&mut self, result: Result<(), String>) {
        self.busy = false;
        self.message = result.err().map(|e| (e, true));
    }

    // ---------------------------------------------------------------- input

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.split.prompt.is_some() {
            self.split.prompt_key(key);
            return;
        }
        if self.filtering {
            match key.code {
                KeyCode::Esc => {
                    self.filtering = false;
                    self.filter.clear();
                }
                KeyCode::Enter => self.filtering = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::ALT) => {
                    self.filter.push(ch)
                }
                KeyCode::Down | KeyCode::Up => {
                    self.filtering = false;
                    self.move_cursor(if key.code == KeyCode::Down { 1 } else { -1 });
                    return;
                }
                _ => return,
            }
            self.rebuild_rows();
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.rebuild_rows();
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Map => Focus::List,
                    Focus::List => Focus::Button,
                    Focus::Button => Focus::Map,
                }
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Map => Focus::Button,
                    Focus::List => Focus::Map,
                    Focus::Button => Focus::List,
                }
            }
            KeyCode::Char('/') => {
                self.tab = Tab::Relays;
                self.filtering = true;
                self.focus = Focus::List;
            }
            KeyCode::Char('s') => {
                self.tab = match self.tab {
                    Tab::Relays => Tab::Split,
                    Tab::Split => Tab::Relays,
                };
                self.focus = Focus::List;
                self.split.refresh();
            }
            KeyCode::Char('a') => {
                self.tab = Tab::Split;
                self.focus = Focus::List;
                self.split.open(PromptKind::Exclude);
            }
            KeyCode::Char('e') => {
                self.tab = Tab::Split;
                self.focus = Focus::List;
                self.split.open(PromptKind::Launch);
            }
            KeyCode::Char('c') => self.connect(),
            KeyCode::Char('d') => self.disconnect(),
            KeyCode::Char(' ') => self.press_button(),
            KeyCode::Char('+' | '=') => self.zoom_center(ZOOM_STEP * ZOOM_STEP),
            KeyCode::Char('-' | '_') => self.zoom_center(1.0 / (ZOOM_STEP * ZOOM_STEP)),
            KeyCode::Char('0') => self.view.reset(),
            _ => match self.focus {
                Focus::Map => self.map_key(key.code),
                Focus::List if self.tab == Tab::Split => self.split.list_key(key.code),
                Focus::List => self.list_key(key.code),
                Focus::Button => {
                    if key.code == KeyCode::Enter {
                        self.press_button();
                    }
                }
            },
        }
    }

    fn map_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Left | KeyCode::Char('h') => self.map_step(-1.0, 0.0),
            KeyCode::Right | KeyCode::Char('l') => self.map_step(1.0, 0.0),
            KeyCode::Up | KeyCode::Char('k') => self.map_step(0.0, 1.0),
            KeyCode::Down | KeyCode::Char('j') => self.map_step(0.0, -1.0),
            KeyCode::Enter => self.connect(),
            _ => {}
        }
    }

    fn list_key(&mut self, code: KeyCode) {
        let Some(node) = self.selected() else { return };
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::PageDown => self.move_cursor(10),
            KeyCode::PageUp => self.move_cursor(-10),
            KeyCode::Home | KeyCode::Char('g') => self.list_state.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => self.move_cursor(isize::MAX / 2),
            KeyCode::Right | KeyCode::Char('l') => {
                if self.has_children(node) && !self.is_open(node) {
                    self.toggle_open(node);
                } else if self.has_children(node) {
                    self.move_cursor(1);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if self.has_children(node) && self.expanded.contains(&node) {
                    self.toggle_open(node);
                } else {
                    let parent = match node {
                        Node::Country(_) => return,
                        Node::City(c, _) => Node::Country(c),
                        Node::Server(c, t, _) => Node::City(c, t),
                    };
                    self.select(parent);
                }
            }
            KeyCode::Enter => {
                if self.has_children(node) {
                    self.toggle_open(node);
                } else {
                    self.connect();
                }
            }
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        let pos = Position::new(ev.column, ev.row);
        if self.split.prompt.is_some() {
            self.split.prompt_mouse(ev);
            return;
        }
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind
            && let Some(i) = self.hit.tabs.iter().position(|t| t.contains(pos))
        {
            self.tab = [Tab::Relays, Tab::Split][i];
            self.focus = Focus::List;
            self.split.refresh();
            return;
        }
        if self.tab == Tab::Split && self.hit.side.contains(pos) {
            if let MouseEventKind::Down(_) = ev.kind {
                self.focus = Focus::List;
            }
            self.split.panel_mouse(ev);
            return;
        }
        match ev.kind {
            MouseEventKind::Moved => {
                self.hover = if self.hit.map.contains(pos) {
                    self.city_at(ev.column, ev.row)
                } else {
                    None
                };
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.hit.button.contains(pos) {
                    self.focus = Focus::Button;
                    self.press_button();
                } else if self.hit.map.contains(pos) {
                    self.focus = Focus::Map;
                    if let Some(city) = self.city_at(ev.column, ev.row) {
                        let double = self.is_double(Click::City(city));
                        self.select_from_map(city);
                        if double {
                            self.connect();
                        }
                    }
                } else if self.hit.list.contains(pos) {
                    self.focus = Focus::List;
                    let i = self.list_state.offset() + (ev.row - self.hit.list.y) as usize;
                    if let Some(&node) = self.rows.get(i) {
                        if self.is_double(Click::Row(node)) {
                            // Undo the fold the first click did, then go.
                            if self.click_toggled {
                                self.toggle_open(node);
                            }
                            self.select(node);
                            self.connect();
                            self.click_toggled = false;
                        } else if self.selected() == Some(node) && self.has_children(node) {
                            self.toggle_open(node);
                            self.click_toggled = true;
                        } else {
                            self.list_state.select(Some(i));
                            self.click_toggled = false;
                        }
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right | MouseButton::Middle)
                if self.hit.map.contains(pos) =>
            {
                self.focus = Focus::Map;
                self.drag = Some((ev.column, ev.row, self.view));
            }
            MouseEventKind::Drag(MouseButton::Right | MouseButton::Middle) => {
                if let Some((col, row, start)) = self.drag {
                    let (dx, dy) = self.cell_size();
                    self.view = start;
                    self.view.lon -= (ev.column as f64 - col as f64) * dx;
                    self.view.lat += (ev.row as f64 - row as f64) * dy;
                    self.view.clamp();
                }
            }
            MouseEventKind::Up(_) => self.drag = None,
            MouseEventKind::ScrollUp if self.hit.map.contains(pos) => {
                self.zoom_at(ev.column, ev.row, ZOOM_STEP)
            }
            MouseEventKind::ScrollDown if self.hit.map.contains(pos) => {
                self.zoom_at(ev.column, ev.row, 1.0 / ZOOM_STEP)
            }
            MouseEventKind::ScrollDown if self.hit.list.contains(pos) => self.move_cursor(3),
            MouseEventKind::ScrollUp if self.hit.list.contains(pos) => self.move_cursor(-3),
            _ => {}
        }
    }
}
