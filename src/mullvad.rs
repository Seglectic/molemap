//! Thin wrapper around the `mullvad` CLI.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;

use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Country {
    pub name: String,
    pub code: String,
    pub cities: Vec<City>,
}

#[derive(Debug, Clone)]
pub struct City {
    pub name: String,
    pub code: String,
    pub lat: f64,
    pub lon: f64,
    pub servers: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    /// "connected", "connecting", "disconnected", "disconnecting", "error"
    pub state: String,
    pub city: Option<String>,
    pub country: Option<String>,
    pub hostname: Option<String>,
    pub ipv4: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

impl Status {
    pub fn is_connected(&self) -> bool {
        self.state == "connected"
    }

    /// Connected or on the way there: the button should offer DISCONNECT.
    pub fn is_up(&self) -> bool {
        matches!(self.state.as_str(), "connected" | "connecting")
    }
}

fn run(args: &[&str]) -> Result<String> {
    let out = Command::new("mullvad")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("failed to run `mullvad` — is the Mullvad app installed?")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("unknown error");
        bail!("mullvad {}: {}", args.join(" "), msg.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn relay_list() -> Result<Vec<Country>> {
    Ok(parse_relay_list(&run(&["relay", "list"])?))
}

/// Parses `mullvad relay list`:
///
/// ```text
/// Albania (al)
/// \tTirana (tia) @ 41.32795°N, 19.81902°W
/// \t\tal-tia-wg-001 (103.124.165.2, 2a04:...) - hosted by iRegister (rented)
/// ```
///
/// The N/W suffixes are misleading: the numbers are already signed latitude and
/// longitude, so only the numbers are used.
pub fn parse_relay_list(text: &str) -> Vec<Country> {
    let mut countries: Vec<Country> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let depth = line.chars().take_while(|c| *c == '\t').count();
        let line = line.trim();
        match depth {
            0 => {
                if let Some((name, code)) = split_name_code(line) {
                    countries.push(Country {
                        name,
                        code,
                        cities: Vec::new(),
                    });
                }
            }
            1 => {
                let Some(country) = countries.last_mut() else {
                    continue;
                };
                let Some((head, coords)) = line.split_once(" @ ") else {
                    continue;
                };
                let Some((name, code)) = split_name_code(head) else {
                    continue;
                };
                let mut nums = coords.split(',').map(parse_coord);
                let (Some(Some(lat)), Some(Some(lon))) = (nums.next(), nums.next()) else {
                    continue;
                };
                country.cities.push(City {
                    name,
                    code,
                    lat,
                    lon,
                    servers: Vec::new(),
                });
            }
            _ => {
                let Some(city) = countries.last_mut().and_then(|c| c.cities.last_mut()) else {
                    continue;
                };
                if let Some(host) = line.split_whitespace().next() {
                    city.servers.push(host.to_string());
                }
            }
        }
    }
    countries
}

/// "Buenos Aires (bue)" -> ("Buenos Aires", "bue")
fn split_name_code(s: &str) -> Option<(String, String)> {
    let open = s.rfind('(')?;
    let close = s[open..].find(')')? + open;
    Some((
        s[..open].trim().to_string(),
        s[open + 1..close].trim().to_string(),
    ))
}

/// " -58.66452°W" -> -58.66452
fn parse_coord(s: &str) -> Option<f64> {
    s.trim()
        .trim_end_matches(|c: char| c.is_alphabetic() || c == '°')
        .parse()
        .ok()
}

pub fn status() -> Result<Status> {
    let v: Value = serde_json::from_str(&run(&["status", "--json"])?)
        .context("could not parse `mullvad status --json`")?;
    let loc = &v["details"]["location"];
    let s = |val: &Value| val.as_str().map(str::to_string);
    Ok(Status {
        state: v["state"].as_str().unwrap_or("unknown").to_string(),
        city: s(&loc["city"]),
        country: s(&loc["country"]),
        hostname: s(&loc["hostname"]),
        ipv4: s(&loc["ipv4"]),
        lat: loc["latitude"].as_f64(),
        lon: loc["longitude"].as_f64(),
    })
}

/// Point the daemon at a country / city / server, then connect.
pub fn connect_to(country: &str, city: Option<&str>, host: Option<&str>) -> Result<()> {
    let mut args = vec!["relay", "set", "location"];
    match host {
        Some(h) => args.push(h),
        None => {
            args.push(country);
            args.extend(city);
        }
    }
    run(&args)?;
    run(&["connect"])?;
    Ok(())
}

pub fn connect() -> Result<()> {
    run(&["connect"]).map(drop)
}

pub fn disconnect() -> Result<()> {
    run(&["disconnect"]).map(drop)
}

/// PIDs excluded from the tunnel (Linux split tunneling is per process).
pub fn split_pids() -> Result<Vec<u32>> {
    Ok(parse_split_list(&run(&["split-tunnel", "list"])?))
}

/// ```text
/// Excluded PIDs:
/// 986820
/// 986822
/// ```
fn parse_split_list(text: &str) -> Vec<u32> {
    text.lines().filter_map(|l| l.trim().parse().ok()).collect()
}

pub fn split_add(pid: u32) -> Result<()> {
    run(&["split-tunnel", "add", &pid.to_string()]).map(drop)
}

pub fn split_delete(pid: u32) -> Result<()> {
    run(&["split-tunnel", "delete", &pid.to_string()]).map(drop)
}

/// Start a shell command outside the tunnel via `mullvad-exclude`.
pub fn launch_excluded(cmd: &str) -> Result<()> {
    spawn_excluded(OsStr::new("sh"), &["-c".into(), cmd.into()], None, None)
}

/// Start a program outside the tunnel via `mullvad-exclude`, detached from our
/// terminal so it outlives molemap. `env`, when given, replaces ours.
pub fn spawn_excluded(
    program: &OsStr,
    args: &[OsString],
    cwd: Option<&Path>,
    env: Option<&[(OsString, OsString)]>,
) -> Result<()> {
    let mut command = Command::new("mullvad-exclude");
    command
        .arg(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = cwd.filter(|d| d.is_dir()) {
        command.current_dir(dir);
    }
    if let Some(env) = env {
        command.env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    }
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn().context("failed to run `mullvad-exclude`")?;
    // Reap it whenever it exits so it doesn't linger as a zombie.
    thread::spawn(move || child.wait());
    Ok(())
}

/// Spawn `mullvad status listen` and ping `tx` on every tunnel state change.
pub fn listen<T: Send + 'static>(tx: Sender<T>, msg: impl Fn() -> T + Send + 'static) {
    thread::spawn(move || {
        let Ok(mut child) = Command::new("mullvad")
            .args(["status", "listen"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return;
        };
        let Some(stdout) = child.stdout.take() else {
            return;
        };
        for _ in BufReader::new(stdout).lines() {
            if tx.send(msg()).is_err() {
                break;
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Albania (al)
\tTirana (tia) @ 41.32795°N, 19.81902°W
\t\tal-tia-wg-001 (103.124.165.2, 2a04:27c0:0:e::f001) - hosted by iRegister (rented)
\t\tal-tia-wg-002 (62.101.165.2, 2a04:27c0:0:f::f001) - hosted by iRegister (rented)

Argentina (ar)
\tBuenos Aires (bue) @ -34.47456°N, -58.66452°W
\t\tar-bue-wg-001 (149.22.83.2, 2a02:6ea0:f002:1::f001) - hosted by DataPacket (rented)
";

    #[test]
    fn parses_relay_list() {
        let c = parse_relay_list(SAMPLE);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].name, "Albania");
        assert_eq!(c[0].code, "al");
        let tirana = &c[0].cities[0];
        assert_eq!(
            (tirana.name.as_str(), tirana.code.as_str()),
            ("Tirana", "tia")
        );
        assert_eq!(tirana.servers, ["al-tia-wg-001", "al-tia-wg-002"]);
        let bue = &c[1].cities[0];
        assert_eq!(bue.name, "Buenos Aires");
        assert_eq!(bue.servers, ["ar-bue-wg-001"]);
    }

    #[test]
    fn parses_split_list() {
        assert_eq!(
            parse_split_list("Excluded PIDs:\n986820\n986822\n"),
            [986820, 986822]
        );
        assert!(parse_split_list("Excluded PIDs:\n").is_empty());
    }

    #[test]
    fn coordinates_are_signed_and_suffixes_ignored() {
        let c = parse_relay_list(SAMPLE);
        // Tirana is east of Greenwich even though the CLI prints "°W".
        assert!((c[0].cities[0].lon - 19.81902).abs() < 1e-9);
        assert!((c[0].cities[0].lat - 41.32795).abs() < 1e-9);
        // Buenos Aires: southern and western hemisphere.
        assert!((c[1].cities[0].lat + 34.47456).abs() < 1e-9);
        assert!((c[1].cities[0].lon + 58.66452).abs() < 1e-9);
    }
}
