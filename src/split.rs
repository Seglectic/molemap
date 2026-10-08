//! Split tunneling: which apps bypass the VPN.
//!
//! On Linux, Mullvad excludes individual processes (and anything they start),
//! so all the daemon can tell us is a list of PIDs. We name those from /proc and
//! group them by program. To exclude a running app, the picker searches only the
//! current user's own processes, and only once something has been typed, so it
//! never dumps every process on the system.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;

use crate::app::Msg;
use crate::mullvad;
use crate::restart::{self, Plan};

/// Per-process split tunneling is how Mullvad does it on Linux; other
/// platforms exclude apps by path through the Mullvad app instead.
pub const SUPPORTED: bool = cfg!(target_os = "linux");

/// Characters to type before the picker shows any processes.
const MIN_QUERY: usize = 2;

/// How long to trust our own change over a daemon list that disagrees.
const PENDING_TTL: Duration = Duration::from_secs(15);

/// Processes sharing a program name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppGroup {
    pub name: String,
    pub pids: Vec<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptKind {
    /// Pick a running app to exclude.
    Exclude,
    /// Type a command to start outside the tunnel.
    Launch,
}

pub enum Prompt {
    /// Search the user's running apps.
    Pick {
        input: String,
        matches: Vec<AppGroup>,
        state: ListState,
    },
    /// Type a command to start outside the tunnel.
    Launch { input: String },
    /// What to do with the app that was picked.
    Choose {
        group: AppGroup,
        plan: Result<Plan, String>,
        choices: Vec<Choice>,
        state: ListState,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    /// Close it and start it again through mullvad-exclude.
    Restart,
    /// Exclude the running processes; open connections stay in the tunnel.
    ExcludeInPlace,
}

/// Screen regions from the last draw.
#[derive(Default)]
pub struct SplitHit {
    pub list: Rect,
    pub add: Rect,
    pub launch: Rect,
    pub remove: Rect,
    pub restart: Rect,
    pub prompt: Rect,
    pub prompt_list: Rect,
    /// One box per choice in the Choose dialog.
    pub choices: Vec<Rect>,
}

pub struct Split {
    pub groups: Vec<AppGroup>,
    pub state: ListState,
    pub prompt: Option<Prompt>,
    pub error: Option<String>,
    pub hit: SplitHit,
    /// PIDs the daemon reported last time we asked.
    daemon: HashSet<u32>,
    /// Changes we made that the daemon hasn't reported back yet: PID →
    /// (excluded?, when). Its list can lag several seconds behind.
    pending: HashMap<u32, (bool, Instant)>,
    loading: bool,
    /// A refresh was asked for mid-load; the in-flight result may be stale.
    again: bool,
    tx: Sender<Msg>,
}

impl Split {
    pub fn new(tx: Sender<Msg>) -> Self {
        Split {
            groups: Vec::new(),
            state: ListState::default(),
            prompt: None,
            error: None,
            hit: SplitHit::default(),
            daemon: HashSet::new(),
            pending: HashMap::new(),
            loading: false,
            again: false,
            tx,
        }
    }

    pub fn excluded_pids(&self) -> HashSet<u32> {
        self.groups
            .iter()
            .flat_map(|g| g.pids.iter().copied())
            .collect()
    }

    pub fn selected(&self) -> Option<&AppGroup> {
        self.state.selected().and_then(|i| self.groups.get(i))
    }

    /// Re-read the exclusion list in the background.
    pub fn refresh(&mut self) {
        if !SUPPORTED {
            return;
        }
        if self.loading {
            self.again = true;
            return;
        }
        self.loading = true;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let list = mullvad::split_pids().map_err(|e| e.to_string());
            let _ = tx.send(Msg::SplitList(list));
        });
    }

    pub fn set_list(&mut self, list: Result<Vec<u32>, String>) {
        self.loading = false;
        if std::mem::take(&mut self.again) {
            self.refresh();
        }
        match list {
            Ok(pids) => {
                self.error = None;
                self.daemon = pids.into_iter().collect();
                self.regroup();
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// We just excluded (or un-excluded) these PIDs.
    pub fn changed(&mut self, pids: Vec<u32>, excluded: bool) {
        let now = Instant::now();
        for pid in pids {
            self.pending.insert(pid, (excluded, now));
        }
        self.regroup();
    }

    /// Rebuild the app list from the daemon's view plus our pending changes.
    fn regroup(&mut self) {
        let daemon = &self.daemon;
        self.pending
            .retain(|pid, (want, at)| at.elapsed() < PENDING_TTL && daemon.contains(pid) != *want);
        let mut set = self.daemon.clone();
        for (pid, &(want, _)) in &self.pending {
            if want {
                set.insert(*pid);
            } else {
                set.remove(pid);
            }
        }
        let mut pids: Vec<u32> = set.into_iter().collect();
        pids.sort_unstable();

        let keep = self.selected().map(|g| g.name.clone());
        self.groups = group(&pids);
        let idx = keep
            .and_then(|name| self.groups.iter().position(|g| g.name == name))
            .or((!self.groups.is_empty()).then_some(0))
            .map(|i| i.min(self.groups.len().saturating_sub(1)));
        self.state.select(idx);
    }

    /// Run a split-tunnel command off the UI thread, report, then refresh.
    fn run(&self, f: impl FnOnce(&Sender<Msg>) -> Result<String, String> + Send + 'static) {
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = f(&tx);
            let _ = tx.send(Msg::Note(result));
            let _ = tx.send(Msg::RefreshSplit);
        });
    }

    pub fn remove_selected(&mut self) {
        let Some(group) = self.selected().cloned() else {
            return;
        };
        self.run(move |tx| {
            let (done, errors): (Vec<_>, Vec<_>) = group
                .pids
                .iter()
                .map(|&p| (p, mullvad::split_delete(p)))
                .partition(|(_, r)| r.is_ok());
            let done: Vec<u32> = done.into_iter().map(|(p, _)| p).collect();
            if done.is_empty() {
                let e = errors.into_iter().find_map(|(_, r)| r.err());
                return Err(e.map_or("couldn't remove exclusion".into(), |e| e.to_string()));
            }
            let _ = tx.send(Msg::SplitChanged(done, false));
            Ok(format!("{} is back inside the VPN", group.name))
        });
    }

    fn exclude(&self, group: AppGroup) {
        self.run(move |tx| {
            let added: Vec<u32> = group
                .pids
                .iter()
                .copied()
                .filter(|&p| mullvad::split_add(p).is_ok())
                .collect();
            if added.is_empty() {
                return Err(format!("couldn't exclude {}", group.name));
            }
            let n = added.len();
            let _ = tx.send(Msg::SplitChanged(added, true));
            let s = if n == 1 { "" } else { "es" };
            Ok(format!(
                "{} ({n} process{s}) now bypasses the VPN · r restarts it to move open connections",
                group.name
            ))
        });
    }

    fn launch(&self, cmd: String) {
        self.run(move |_| {
            mullvad::launch_excluded(&cmd).map_err(|e| e.to_string())?;
            // We don't know the new PID; give mullvad-exclude a moment to
            // register it before the refresh. Polling picks up any stragglers.
            thread::sleep(Duration::from_millis(800));
            Ok(format!("started `{cmd}` outside the VPN"))
        });
    }

    fn restart(&self, plan: Plan) {
        self.run(move |tx| {
            let _ = tx.send(Msg::Note(Ok(format!("closing {}…", plan.name))));
            restart::execute(&plan)?;
            // Give mullvad-exclude a moment to register the new process.
            thread::sleep(Duration::from_millis(1500));
            Ok(format!("{} restarted outside the VPN", plan.name))
        });
    }

    // ---------------------------------------------------------------- prompt

    pub fn open(&mut self, kind: PromptKind) {
        if !SUPPORTED {
            return;
        }
        self.prompt = Some(match kind {
            PromptKind::Exclude => Prompt::Pick {
                input: String::new(),
                matches: Vec::new(),
                state: ListState::default(),
            },
            PromptKind::Launch => Prompt::Launch {
                input: String::new(),
            },
        });
    }

    /// Ask what to do with an app: restart it outside the VPN, or (if it
    /// isn't already) exclude it as it runs.
    pub fn open_choose(&mut self, group: AppGroup) {
        if !SUPPORTED {
            return;
        }
        let excluded = self.excluded_pids();
        let already = group.pids.iter().all(|p| excluded.contains(p));
        let plan = restart::plan(&group);
        let mut choices = Vec::new();
        if plan.is_ok() {
            choices.push(Choice::Restart);
        }
        if !already {
            choices.push(Choice::ExcludeInPlace);
        }
        let mut state = ListState::default();
        state.select((!choices.is_empty()).then_some(0));
        self.prompt = Some(Prompt::Choose {
            group,
            plan,
            choices,
            state,
        });
    }

    fn update_matches(&mut self) {
        let Some(Prompt::Pick {
            input,
            matches,
            state,
        }) = self.prompt.as_mut()
        else {
            return;
        };
        let query = input.trim().to_lowercase();
        *matches = if query.chars().count() < MIN_QUERY {
            Vec::new()
        } else {
            let mut m: Vec<AppGroup> = group(&procs::own_pids())
                .into_iter()
                .filter(|g| g.name.to_lowercase().contains(&query))
                .collect();
            // Names starting with the query first, then alphabetical.
            m.sort_by_key(|g| {
                (
                    !g.name.to_lowercase().starts_with(&query),
                    g.name.to_lowercase(),
                )
            });
            m
        };
        state.select((!matches.is_empty()).then_some(0));
    }

    fn confirm(&mut self) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        match prompt {
            Prompt::Pick {
                ref matches,
                ref state,
                ..
            } => match state.selected().and_then(|i| matches.get(i)) {
                Some(group) => self.open_choose(group.clone()),
                None => self.prompt = Some(prompt),
            },
            Prompt::Launch { ref input } => {
                let cmd = input.trim().to_string();
                if cmd.is_empty() {
                    self.prompt = Some(prompt);
                } else {
                    self.launch(cmd);
                }
            }
            Prompt::Choose {
                group,
                plan,
                choices,
                state,
            } => match (state.selected().and_then(|i| choices.get(i)), plan) {
                (Some(Choice::Restart), Ok(plan)) => self.restart(plan),
                (Some(Choice::ExcludeInPlace), _) => self.exclude(group),
                // Nothing we can do; Enter closes the dialog.
                _ => {}
            },
        }
    }

    fn prompt_move(&mut self, delta: isize) {
        let (len, state) = match self.prompt.as_mut() {
            Some(Prompt::Pick { matches, state, .. }) => (matches.len(), state),
            Some(Prompt::Choose { choices, state, .. }) => (choices.len(), state),
            _ => return,
        };
        if len > 0 {
            let i = state.selected().unwrap_or(0) as isize + delta;
            state.select(Some(i.clamp(0, len as isize - 1) as usize));
        }
    }

    pub fn prompt_key(&mut self, key: KeyEvent) {
        let input = match self.prompt.as_mut() {
            Some(Prompt::Pick { input, .. } | Prompt::Launch { input }) => Some(input),
            Some(Prompt::Choose { .. }) => None,
            None => return,
        };
        match key.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Enter => self.confirm(),
            KeyCode::Up => self.prompt_move(-1),
            KeyCode::Down => self.prompt_move(1),
            KeyCode::Backspace => {
                if let Some(input) = input {
                    input.pop();
                    self.update_matches();
                }
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
            {
                if let Some(input) = input {
                    input.push(ch);
                    self.update_matches();
                }
            }
            _ => {}
        }
    }

    pub fn prompt_mouse(&mut self, ev: MouseEvent) {
        let pos = Position::new(ev.column, ev.row);
        let in_list = self.hit.prompt_list.contains(pos);
        let choice = self.hit.choices.iter().position(|r| r.contains(pos));
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) if choice.is_some() => {
                if let Some(Prompt::Choose { state, .. }) = self.prompt.as_mut() {
                    state.select(choice);
                    self.confirm();
                }
            }
            MouseEventKind::Down(MouseButton::Left) if in_list => {
                if let Some(Prompt::Pick { matches, state, .. }) = self.prompt.as_mut() {
                    let i = state.offset() + (ev.row - self.hit.prompt_list.y) as usize;
                    if i < matches.len() {
                        state.select(Some(i));
                        self.confirm();
                    }
                }
            }
            MouseEventKind::Down(_) if !self.hit.prompt.contains(pos) => self.prompt = None,
            MouseEventKind::ScrollDown if in_list => self.prompt_move(1),
            MouseEventKind::ScrollUp if in_list => self.prompt_move(-1),
            _ => {}
        }
    }

    // ---------------------------------------------------------------- panel

    /// Offer to restart the selected (already excluded) app, so connections
    /// it opened before being excluded move out of the tunnel too.
    pub fn restart_selected(&mut self) {
        if let Some(group) = self.selected().cloned() {
            self.open_choose(group);
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.groups.is_empty() {
            return;
        }
        let i = self.state.selected().unwrap_or(0) as isize + delta;
        self.state
            .select(Some(i.clamp(0, self.groups.len() as isize - 1) as usize));
    }

    pub fn list_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Delete | KeyCode::Backspace | KeyCode::Char('x') => self.remove_selected(),
            KeyCode::Char('r') => self.restart_selected(),
            _ => {}
        }
    }

    pub fn panel_mouse(&mut self, ev: MouseEvent) {
        let pos = Position::new(ev.column, ev.row);
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.hit.add.contains(pos) {
                    self.open(PromptKind::Exclude);
                } else if self.hit.launch.contains(pos) {
                    self.open(PromptKind::Launch);
                } else if self.hit.remove.contains(pos) {
                    self.remove_selected();
                } else if self.hit.restart.contains(pos) {
                    self.restart_selected();
                } else if self.hit.list.contains(pos) {
                    let i = self.state.offset() + (ev.row - self.hit.list.y) as usize;
                    if i < self.groups.len() {
                        self.state.select(Some(i));
                    }
                }
            }
            MouseEventKind::ScrollDown => self.move_cursor(1),
            MouseEventKind::ScrollUp => self.move_cursor(-1),
            _ => {}
        }
    }
}

/// Group PIDs by program name, dropping ones that have exited.
fn group(pids: &[u32]) -> Vec<AppGroup> {
    let mut by_name: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for &pid in pids {
        if let Some(name) = procs::name(pid) {
            by_name.entry(name).or_default().push(pid);
        }
    }
    by_name
        .into_iter()
        .map(|(name, pids)| AppGroup { name, pids })
        .collect()
}

#[cfg(target_os = "linux")]
mod procs {
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    /// Program name: the executable's file name, or `comm` when the
    /// executable can't be read (another user's process).
    pub fn name(pid: u32) -> Option<String> {
        if let Ok(exe) = fs::read_link(format!("/proc/{pid}/exe"))
            && let Some(file) = exe.file_name()
        {
            // An upgraded binary shows up as "name (deleted)".
            return Some(
                file.to_string_lossy()
                    .trim_end_matches(" (deleted)")
                    .to_string(),
            );
        }
        let comm = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        Some(comm.trim().to_string())
    }

    /// The current user's processes, minus molemap itself.
    pub fn own_pids() -> Vec<u32> {
        let Ok(me) = fs::metadata("/proc/self").map(|m| m.uid()) else {
            return Vec::new();
        };
        let this = std::process::id();
        let Ok(dir) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        dir.flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .filter(|&pid| pid != this)
            .filter(|pid| fs::metadata(format!("/proc/{pid}")).is_ok_and(|m| m.uid() == me))
            .collect()
    }
}

#[cfg(not(target_os = "linux"))]
mod procs {
    pub fn name(_pid: u32) -> Option<String> {
        None
    }

    pub fn own_pids() -> Vec<u32> {
        Vec::new()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn groups_live_processes_by_name() {
        let me = std::process::id();
        let groups = group(&[me, me, u32::MAX]);
        assert_eq!(groups.len(), 1, "dead PIDs are dropped");
        assert_eq!(groups[0].pids, [me, me]);
        assert!(!groups[0].name.is_empty());
    }

    #[test]
    fn our_changes_win_over_a_lagging_daemon() {
        let me = std::process::id();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut split = Split::new(tx);
        let shown = |s: &Split| s.excluded_pids().contains(&me);

        // Excluded by us; the daemon hasn't caught up yet.
        split.set_list(Ok(vec![]));
        split.changed(vec![me], true);
        assert!(shown(&split));
        split.set_list(Ok(vec![]));
        assert!(shown(&split), "stale list must not undo our exclusion");

        // Once the daemon agrees, it's in charge again.
        split.set_list(Ok(vec![me]));
        split.set_list(Ok(vec![]));
        assert!(!shown(&split));

        // Same for putting an app back in the tunnel.
        split.set_list(Ok(vec![me]));
        split.changed(vec![me], false);
        split.set_list(Ok(vec![me]));
        assert!(!shown(&split), "stale list must not undo our removal");
    }

    #[test]
    fn own_processes_exclude_ourselves() {
        assert!(!procs::own_pids().contains(&std::process::id()));
    }
}
