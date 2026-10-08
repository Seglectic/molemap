# molemap

A terminal control panel for [Mullvad VPN](https://mullvad.net). It shows every
Mullvad relay on a zoomable world map. Click a city, or pick one from the list,
and hit the big button to connect.

```
 ★ MOLEMAP   MULLVAD VPN CONTROL PANEL                      536 RELAYS · 91 CITIES
╔╡ WORLD MAP · ×6.0 ╞═══════════════════════════════════╗╔╡ RELAYS ╞═══════════════╗
║  ⡀⠈Gothenburg ⢸⠖⠁   ⢀⡖⠁ ⠘⠤⠤⠓     ⠈⣗                    ║║▾ SWEDEN se              ║
║ ⢀⡼Copenhagen    ⢸⡠⠤⠤⠤⠤⠤⠤⢤⣀⡀   ⢘⣆⡀                      ║║  ▸ Gothenburg (9)       ║
║ Amsterdam   ★ Berlin, DE      Warsaw                   ║║  ▸ Malmö (11)           ║
║ Brussels  Frankfurt ⢀⡈Prague       ⠉⠋⠉⠋⠓⠒⠲⠇Kyiv        ║║  ▸ Stockholm (17)       ║
╚═══════════════════════════════════════════════════════╝╚═════════════════════════╝
╔╡ STATUS ╞═════════════════════════════════════════════╗┏━━━━━━━━━━━━━━━━━━━━━━━━━┓
║ STATE    ● DISCONNECTED  UNPROTECTED                  ║┃            ★            ┃
║ LOCATION Somewhere, Earth                             ║┃      C O N N E C T      ┃
║ TARGET   Berlin, Germany                              ║┃       tunnel down       ┃
╚═══════════════════════════════════════════════════════╝┗━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

## Features

- **World map of relays.** Every Mullvad relay city is plotted on a braille-drawn
  map with coastlines and country borders. You can zoom with the scroll wheel and
  pan by right-dragging. Zoomed in, cities get name labels.
- **Mouse and keyboard.** Click a city or a list row to select it, and
  double-click to connect straight away. Everything also works from the keyboard.
- **Country → city → server list** with search (`/`), so you can connect to a
  whole country, a city, or one specific server.
- **One big button.** It is green to connect and red to disconnect, and it
  follows the tunnel state live.
- **Your location.** While disconnected, ⌂ marks where you are. While
  connected, ◉ marks the relay you're going through.
- **Split tunneling (Linux).** See which apps bypass the VPN. You can move an
  app that's already running out of the VPN by restarting it in one step,
  without hunting down its launcher. You can also launch apps excluded, or put
  them back in the tunnel.

## Requirements

- The [Mullvad VPN app](https://mullvad.net/download), installed and logged in.
  molemap drives the `mullvad` command-line tool that comes with it, so `mullvad`
  must be on your `PATH`.
- A terminal with Unicode and true colour support, at least 70×22. Mouse support
  is recommended.

## Install

**Prebuilt binaries:** download one for Linux, macOS or Windows from the
[releases page](https://github.com/Seglectic/molemap/releases), unpack it, and
put `molemap` somewhere on your `PATH`.

**With Cargo** (Rust 1.88 or newer):

```sh
cargo install --git https://github.com/Seglectic/molemap
```

**From source:**

```sh
git clone https://github.com/Seglectic/molemap
cd molemap
cargo build --release   # binary at target/release/molemap
```

## Usage

Run `molemap`.

| Action | Keyboard | Mouse |
|---|---|---|
| Move focus between map, list and button | `Tab` / `Shift+Tab` | click a panel |
| Pick a city on the map | arrows / `hjkl` jump to the nearest city in that direction | click |
| Browse the list | arrows / `hjkl`, `Enter` folds and unfolds | click, wheel scrolls |
| Search the list | `/`, then type; `Esc` clears | |
| Connect to the selection | `c` (or `Enter` on the map) | double-click a city or row |
| Disconnect | `d` | |
| Press the big button | `Space` | click it |
| Zoom | `+` / `-`, `0` resets | scroll wheel over the map |
| Pan | | right-drag (or middle-drag) |
| Show split tunneling | `s` | click the SPLIT tab |
| Exclude a running app | `a` | click **+ EXCLUDE RUNNING APP** |
| Launch an app excluded | `e` | click **▶ LAUNCH EXCLUDED** |
| Restart an excluded app so its open connections move too | `r` | click **↻ RESTART OUTSIDE VPN** |
| Put an app back in the VPN | `Del` / `x` | click **✕ PUT BACK IN VPN** |
| Quit | `q` / `Ctrl+C` | |

Connecting runs `mullvad relay set location …` and then `mullvad connect`. That
means the location you pick also becomes your saved location in the Mullvad app.

## Split tunneling

On Linux, Mullvad excludes individual processes from the tunnel (plus anything
they start), not apps by path. The SPLIT tab shows exactly what the daemon has
excluded, grouped by program name. It never lists other processes.

- **Move a running app out of the VPN** (`a`): type part of a program name.
  Nothing appears until you've typed two characters, and only your own
  processes are searched. Then choose one of:
  - **Restart outside the VPN.** molemap works out how the app was started,
    asks it to quit, waits for it to close (and force-closes it after 10
    seconds), then starts it again through `mullvad-exclude`. It shows you the
    exact command first.
    - **Flatpak apps** come back with `flatpak run <app-id>` and their original
      arguments.
    - **AppImages** come back from their original `.AppImage` file.
    - **Everything else** comes back with its original command, working
      directory and environment.

    Everything the app does then bypasses the VPN. That includes long-lived
    connections like Discord calls and chat.
  - **Exclude without restarting.** The running processes are excluded where
    they are. New connections bypass the VPN, but connections that are already
    open stay inside it until the app reconnects. Press `r` in the SPLIT tab
    later to restart the app and move those too.
- **Launch excluded** (`e`): runs a command through `mullvad-exclude`, so the app
  is outside the tunnel from the start.
- Exclusions last until the process exits. Mullvad doesn't remember them, so
  they don't survive a restart of the app.

On macOS and Windows, Mullvad excludes apps by path through its own settings
screen. molemap doesn't manage that yet.

## How it works

molemap is a thin front end. It reads relays from `mullvad relay list` and the
tunnel state from `mullvad status --json`, and it listens to
`mullvad status listen` for live updates. Split tunneling uses
`mullvad split-tunnel` and `mullvad-exclude`, and reads process names from
`/proc`. It doesn't talk to Mullvad's servers or the daemon directly, and it
never sees your account.

The UI is built with [ratatui](https://ratatui.rs).

## Credits

- Coastline and border data: [Natural Earth](https://www.naturalearthdata.com)
  (1:50m, public domain), stored in `assets/` in a compact binary format.
- molemap is an independent project. It is not affiliated with or endorsed by
  Mullvad VPN AB. "Mullvad" is a trademark of Mullvad VPN AB.

## License

[MIT](LICENSE)
