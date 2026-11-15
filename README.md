<div align="center">

<img src="apps/desktop/src-tauri/icons/128x128@2x.png" width="112" alt="PowerKit icon">

# PowerKit

**Windows power-user utilities, written in Rust.**

Window zones, a launcher, key remapping, a color picker, bulk rename, a shortcut guide and more,
in one small background process. The goal is to cover every Microsoft PowerToys utility.

![Windows 10 and 11](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D4?logo=windows&logoColor=white)
![Rust stable](https://img.shields.io/badge/Rust-stable-B7410E?logo=rust&logoColor=white)
![License MIT](https://img.shields.io/badge/license-MIT-2EA44F)

</div>

---

## Utilities

| | Utility | What it does | Default shortcut |
|---|---|---|---|
| 🪟 | **FancyZones** | Snap windows into custom zone layouts. Drag with Shift held, or send a window by keyboard. | `Win+Ctrl+Alt+←/→`, `Win+Ctrl+Alt+1…9` |
| 🚀 | **PowerKit Run** | A launcher for apps, files, calculations and commands. | `Alt+Space` |
| ⌨️ | **Keyboard Manager** | Remap keys and shortcuts, everywhere or only in chosen apps. | — |
| 🎨 | **Color Picker** | Pick any color on screen and copy it as HEX, RGB, HSL and more. | `Win+Shift+C` |
| ✏️ | **PowerRename** | Bulk rename with search and replace, regular expressions and a live preview. | Explorer menu |
| 📌 | **Always On Top** | Pin a window above all others, with a colored border. | `Win+Ctrl+T` |
| ☕ | **Awake** | Keep the PC awake indefinitely, for a while, or until a set time. | Tray |
| 🌗 | **Light Switch** | Switch between light and dark mode on a schedule or by shortcut. | `Win+Ctrl+Shift+D` |
| 🔤 | **Quick Accent** | Hold a letter, then press Space or an arrow key to type `é`, `ñ`, `ß`… | Hold a letter |
| 🖱️ | **Mouse utilities** | Find My Mouse, a click highlighter, crosshairs with a gliding cursor, and Mouse Jump. | `Ctrl` ×2, `Win+Shift+H`, `Win+Alt+P`, `Win+Shift+D` |
| ✂️ | **Crop and Lock** | Crop a window into a live thumbnail, or into a smaller window that still takes input. | `Win+Ctrl+Shift+T`, `Win+Ctrl+Shift+R` |
| ❓ | **Shortcut Guide** | Hold the Windows key to see the shortcuts for Windows, PowerKit and the focused app, with numbers on the taskbar buttons. | Hold `Win`, or `Win+Shift+/` |
| ✋ | **Grab and Move** | Hold `Win` (or `Alt`) and drag anywhere in a window to move it; drag with the right button to resize it. | `Win` + drag |
| 🔀 | **Window Hopper** | Switch between the windows of the focused app, including minimized ones. | ``Alt+` ``, ``Alt+Shift+` `` |

Every shortcut can be changed or removed in the configuration, and each utility can be
turned on or off from the tray icon.

### Coming next

Hosts File Editor · Environment Variables · Registry Preview ·
New+ · File Locksmith · Image Resizer · Text Extractor · Power Display · Screen Ruler · ZoomIt ·
Workspaces · Peek · Advanced Paste · Command Not Found

## Install

1. Download `powerkit.exe` from the [latest release](https://github.com/siddarth1872004/PowerKit/releases/latest).
2. Run it. PowerKit lives in the notification area. Right-click the tray icon to turn
   utilities on or off, check for updates or quit.

To keep settings next to the program (for a USB stick, say), put an empty file named
`portable` beside `powerkit.exe`.

### Updates

PowerKit checks GitHub for a new release once a day, starting two minutes after it starts,
and tells you when one is out. Choose **Install update** in the tray menu and it downloads
the new version, swaps it in and restarts. You can also choose **Check for updates** at any
time. To turn off the daily check, set `general.check_for_updates` to `false`.

The check sends a plain request to the GitHub API, with nothing that identifies you or
your PC.

## Configuration

Choose **Open settings** in the tray menu (or click the tray icon). Pick a utility on the
left, select a setting, type its new value in the box underneath and press Enter. Double-click
an on/off setting to flip it. Changes apply at once.

Settings are stored in `%APPDATA%\PowerKit\config.json` (or `data\config.json` in portable
mode). PowerKit reads the file when it starts, so quit PowerKit before editing it by hand.
Invalid values are repaired, with a warning in the log, and the last good file is kept as
`config.backup.json`.

## Command line

```text
powerkit               start, or open settings if it is already running
powerkit --settings    start and open settings
powerkit --quit        ask the running PowerKit to exit
powerkit --version     print the version
```

## Build from source

You need the stable Rust toolchain. To build on Windows:

```powershell
cargo build --release -p powerkit-host --bin powerkit
```

To cross-check from Linux or WSL:

```bash
rustup target add x86_64-pc-windows-gnu
cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu
cargo test --workspace   # the platform-independent logic
```

### Project layout

```text
apps/background         powerkit.exe: tray icon, global shortcuts, IPC, updater
crates/powerkit-core    shared types: shortcuts, keys, rectangles, events
crates/powerkit-config  the configuration schema, validation and storage
crates/powerkit-platform  every Win32 call, and the only crate with unsafe code
crates/powerkit-plugin-api  the Module trait each utility implements
modules/*               one crate per utility, with its logic tested without Windows
```

## Releasing

Set the version in the root `Cargo.toml`, commit, then push a tag with the same version:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

The release workflow tests and builds `powerkit.exe` on Windows and publishes it, which is
what the updater looks for. It stops if the tag and the `Cargo.toml` version differ.

## License

[MIT](LICENSE)
