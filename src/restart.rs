//! Restarting a running app outside the VPN.
//!
//! Excluding a running process only affects connections it opens afterwards,
//! so long-lived ones (Discord's voice and gateway, game sessions) stay in the
//! tunnel. Restarting the app through `mullvad-exclude` moves everything. We
//! work out from /proc how the app was started, ask it to quit, wait until it
//! has, and start it again the same way.

// Only Linux has an implementation; elsewhere the types exist but go unused.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::OsString;
use std::path::PathBuf;

use crate::split::AppGroup;

/// How a process was started, so it can be started again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Native,
    /// Flatpak app ID, relaunched with `flatpak run`.
    Flatpak(String),
    /// Relaunched from the original `.AppImage` file.
    AppImage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    /// The original environment; `None` inherits ours (Flatpak, whose own
    /// environment is the sandbox's).
    pub env: Option<Vec<(OsString, OsString)>>,
    pub source: Source,
}

impl Launch {
    /// The command line, for showing before we run it.
    pub fn describe(&self) -> String {
        std::iter::once(&self.program)
            .chain(&self.args)
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A process, identified by PID plus start time so a reused PID is never
/// mistaken for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    started: u64,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub name: String,
    /// Every process of the app; all must be gone before relaunching.
    pub procs: Vec<Proc>,
    /// Top-level processes, which get asked to quit.
    pub mains: Vec<Proc>,
    pub launches: Vec<Launch>,
}

pub use imp::{execute, plan};

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::HashSet;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{AppGroup, Launch, Plan, Proc, Source};
    use crate::mullvad;
    use crate::split::procs::{self, flatpak_id};

    /// How long the app gets to quit by itself before it's killed.
    const QUIT_TIMEOUT: Duration = Duration::from_secs(10);
    const KILL_TIMEOUT: Duration = Duration::from_secs(3);

    /// `/proc/<pid>/stat` fields after the `(comm)`: state, ppid, … starttime.
    fn stat(pid: u32) -> Option<(char, u32, u64)> {
        let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest: Vec<&str> = s[s.rfind(')')? + 1..].split_whitespace().collect();
        let state = rest.first()?.chars().next()?;
        // Field 22 overall is starttime; `rest` starts at field 3.
        Some((
            state,
            rest.get(1)?.parse().ok()?,
            rest.get(19)?.parse().ok()?,
        ))
    }

    /// Still the same process, and not a zombie.
    fn alive(p: Proc) -> bool {
        stat(p.pid).is_some_and(|(state, _, started)| state != 'Z' && started == p.started)
    }

    fn signal(p: Proc, sig: libc::c_int) {
        if alive(p) {
            // SAFETY: kill(2) has no memory-safety requirements.
            unsafe { libc::kill(p.pid as libc::pid_t, sig) };
        }
    }

    /// A process's arguments. Chromium and Electron apps (Discord, Slack,
    /// VS Code…) overwrite theirs with one space-separated string, so a single
    /// argument with spaces that isn't a real path gets split back up.
    fn argv(pid: u32) -> Vec<OsString> {
        let argv = nul_list(&fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default());
        unsquash(argv)
    }

    fn unsquash(argv: Vec<OsString>) -> Vec<OsString> {
        match argv.as_slice() {
            [only] if only.as_bytes().contains(&b' ') && !Path::new(only).exists() => only
                .as_bytes()
                .split(|b| *b == b' ')
                .filter(|s| !s.is_empty())
                .map(|s| OsString::from_vec(s.to_vec()))
                .collect(),
            _ => argv,
        }
    }

    fn nul_list(bytes: &[u8]) -> Vec<OsString> {
        bytes
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| OsString::from_vec(s.to_vec()))
            .collect()
    }

    /// Chromium/Electron helper processes are started by the app itself.
    fn is_helper(argv: &[OsString]) -> bool {
        argv.iter()
            .skip(1)
            .any(|a| a.as_bytes().starts_with(b"--type="))
    }

    pub fn plan(group: &AppGroup) -> Result<Plan, String> {
        let in_group: HashSet<u32> = group.pids.iter().copied().collect();
        let mut procs = Vec::new();
        // (process, parent PID, arguments)
        let mut members = Vec::new();
        for &pid in &group.pids {
            let Some((_, ppid, started)) = stat(pid) else {
                continue;
            };
            let p = Proc { pid, started };
            procs.push(p);
            members.push((p, ppid, argv(pid)));
        }

        // A Flatpak app is restarted with `flatpak run`, taking its arguments
        // from the app's own top process inside the sandbox; the sandbox
        // plumbing around it (bwrap, wrapper scripts, proxies) isn't relaunched.
        if let Some(id) = group.pids.iter().find_map(|&p| flatpak_id(p)) {
            let app = procs::flatpak_app_name(&group.pids);
            let mains: Vec<&(Proc, u32, Vec<OsString>)> = members
                .iter()
                .filter(|(p, ppid, argv)| {
                    let name = procs::name(p.pid);
                    !argv.is_empty()
                        && !is_helper(argv)
                        && name.is_some()
                        && name == app
                        && procs::name(*ppid) != name
                        && flatpak_id(p.pid).as_deref() == Some(id.as_str())
                })
                .collect();
            let Some((_, _, argv)) = mains.first() else {
                return Err(format!("couldn't find {}'s main process", group.name));
            };
            let mut run: Vec<OsString> = vec!["run".into(), id.clone().into()];
            run.extend(argv[1..].iter().cloned());
            return Ok(Plan {
                name: group.name.clone(),
                procs,
                mains: mains.iter().map(|(p, _, _)| *p).collect(),
                launches: vec![Launch {
                    program: "flatpak".into(),
                    args: run,
                    cwd: None,
                    env: None,
                    source: Source::Flatpak(id),
                }],
            });
        }

        let mut mains = Vec::new();
        let mut launches: Vec<Launch> = Vec::new();
        for (p, ppid, argv) in &members {
            if in_group.contains(ppid) || argv.is_empty() || is_helper(argv) {
                continue;
            }
            let launch = launch_for(p.pid, argv)?;
            mains.push(*p);
            if !launches.contains(&launch) {
                launches.push(launch);
            }
        }
        if mains.is_empty() {
            return Err(format!("couldn't tell how {} was started", group.name));
        }
        Ok(Plan {
            name: group.name.clone(),
            procs,
            mains,
            launches,
        })
    }

    fn launch_for(pid: u32, argv: &[OsString]) -> Result<Launch, String> {
        let args = argv[1..].to_vec();

        let environ = fs::read(format!("/proc/{pid}/environ"))
            .map_err(|_| "can't read its environment".to_string())?;
        let env: Vec<(OsString, OsString)> = nul_list(&environ)
            .into_iter()
            .filter_map(|kv| {
                let b = kv.as_bytes();
                let i = b.iter().position(|c| *c == b'=')?;
                Some((
                    OsStr::from_bytes(&b[..i]).into(),
                    OsStr::from_bytes(&b[i + 1..]).into(),
                ))
            })
            .collect();
        let cwd = fs::read_link(format!("/proc/{pid}/cwd")).ok();

        if let Some((_, image)) = env.iter().find(|(k, _)| k == "APPIMAGE") {
            return Ok(Launch {
                program: image.clone(),
                args,
                cwd,
                env: Some(env),
                source: Source::AppImage,
            });
        }

        let exe = fs::read_link(format!("/proc/{pid}/exe"))
            .map_err(|_| "can't read its executable".to_string())?;
        if exe.as_os_str().as_bytes().ends_with(b" (deleted)") {
            return Err("it was updated after it started; restart it yourself once".into());
        }
        // Prefer argv[0] when it's an absolute path that exists, so symlinked
        // launchers behave the same as before.
        let argv0 = Path::new(&argv[0]);
        let program = if argv0.is_absolute() && argv0.exists() {
            argv[0].clone()
        } else {
            exe.into_os_string()
        };
        Ok(Launch {
            program,
            args,
            cwd,
            env: Some(env),
            source: Source::Native,
        })
    }

    fn wait_gone(procs: &[Proc], timeout: Duration) -> bool {
        let start = Instant::now();
        while procs.iter().any(|&p| alive(p)) {
            if start.elapsed() > timeout {
                return false;
            }
            thread::sleep(Duration::from_millis(100));
        }
        true
    }

    /// Ask the app to quit (SIGTERM), kill whatever is left after a while,
    /// then start it again outside the tunnel.
    pub fn execute(plan: &Plan) -> Result<(), String> {
        for &p in &plan.mains {
            signal(p, libc::SIGTERM);
        }
        if !wait_gone(&plan.procs, QUIT_TIMEOUT) {
            for &p in &plan.procs {
                signal(p, libc::SIGKILL);
            }
            if !wait_gone(&plan.procs, KILL_TIMEOUT) {
                return Err(format!("{} didn't quit", plan.name));
            }
        }
        // Let single-instance locks and sockets clear before starting again.
        thread::sleep(Duration::from_millis(500));
        for l in &plan.launches {
            mullvad::spawn_excluded(&l.program, &l.args, l.cwd.as_deref(), l.env.as_deref())
                .map_err(|e| format!("{} closed, but restarting it failed: {e}", plan.name))?;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn proc_of(pid: u32) -> Option<Proc> {
            stat(pid).map(|(_, _, started)| Proc { pid, started })
        }

        #[test]
        fn reads_our_own_stat() {
            let me = proc_of(std::process::id()).unwrap();
            assert!(alive(me));
            assert!(
                !alive(Proc {
                    started: me.started + 1,
                    ..me
                }),
                "reused PID"
            );
        }

        #[test]
        fn plans_a_native_relaunch() {
            let mut child = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap();
            let pid = child.id();
            // Right after spawn the child may still be mid-exec.
            for _ in 0..200 {
                if argv(pid) == [OsString::from("sleep"), "30".into()] {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            let group = AppGroup {
                name: "sleep".into(),
                pids: vec![pid],
            };
            let plan = plan(&group).unwrap();
            assert_eq!(plan.mains.len(), 1);
            let l = &plan.launches[0];
            assert_eq!(l.source, Source::Native);
            assert_eq!(l.args, [OsString::from("30")]);
            assert!(l.env.as_ref().is_some_and(|e| !e.is_empty()));
            assert!(l.describe().ends_with("sleep 30"));
            child.kill().unwrap();
            child.wait().unwrap();
            assert!(!alive(plan.procs[0]));
        }

        #[test]
        fn splits_electron_style_command_lines() {
            let squashed = vec![OsString::from("/app/discord/Discord --start-minimized")];
            assert_eq!(
                unsquash(squashed),
                [
                    OsString::from("/app/discord/Discord"),
                    "--start-minimized".into()
                ]
            );
            let helper = unsquash(vec!["/app/discord/Discord --type=zygote".into()]);
            assert!(is_helper(&helper));
            // A single argument without spaces is left alone.
            let real = vec![OsString::from("/tmp")];
            assert_eq!(unsquash(real.clone()), real);
        }

        #[test]
        fn helpers_are_not_mains() {
            assert!(is_helper(&["/app/x".into(), "--type=renderer".into()]));
            assert!(!is_helper(&["/app/x".into(), "--start-minimized".into()]));
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::{AppGroup, Plan};

    pub fn plan(_group: &AppGroup) -> Result<Plan, String> {
        Err("restarting apps is Linux-only for now".into())
    }

    pub fn execute(_plan: &Plan) -> Result<(), String> {
        Err("restarting apps is Linux-only for now".into())
    }
}
