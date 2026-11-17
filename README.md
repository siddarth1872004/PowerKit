<div align="center">

<img src="apps/desktop/src-tauri/icons/128x128@2x.png" width="112" alt="PowerKit icon">

# PowerKit

**Windows power-user utilities, written in Rust.**

Window zones, a launcher, key remapping, a color picker, bulk rename, a shortcut guide and more,
in one small background process. The goal is to cover every Microsoft PowerToys utility.

**[Website](https://siddarth1872004.github.io/PowerKit/) · [Download](https://github.com/siddarth1872004/PowerKit/releases/latest/download/powerkit.exe)**

![Windows 10 and 11](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D4?logo=windows&logoColor=white)
![Rust stable](https://img.shields.io/badge/Rust-stable-B7410E?logo=rust&logoColor=white)
![License MIT](https://img.shields.io/badge/license-MIT-2EA44F)

</div>

---

## Utilities

| | Utility | What it does | Default shortcut |
|---|---|---|---|
| <img src="assets/modules/fancy_zones.png" width="48" height="48" alt=""> | **FancyZones** | Snap windows into custom zone layouts. Drag with Shift held, or send a window by keyboard. | `Win+Ctrl+Alt+←/→`, `Win+Ctrl+Alt+1…9` |
| <img src="assets/modules/launcher.png" width="48" height="48" alt=""> | **PowerKit Run** | A launcher for apps, files, calculations and commands. | `Alt+Space` |
| <img src="assets/modules/keyboard_manager.png" width="48" height="48" alt=""> | **Keyboard Manager** | Remap keys and shortcuts, everywhere or only in chosen apps. | — |
| <img src="assets/modules/color_picker.png" width="48" height="48" alt=""> | **Color Picker** | Pick any color on screen and copy it as HEX, RGB, HSL and more. | `Win+Shift+C` |
| <img src="assets/modules/power_rename.png" width="48" height="48" alt=""> | **PowerRename** | Bulk rename with search and replace, regular expressions, case changes and numbering, with a live preview and undo. | Explorer menu |
| <img src="assets/modules/always_on_top.png" width="48" height="48" alt=""> | **Always On Top** | Pin a window above all others, with a colored border. | `Win+Ctrl+T` |
| <img src="assets/modules/awake.png" width="48" height="48" alt=""> | **Awake** | Keep the PC awake indefinitely, for a while, or until a set time. Its own tray icon (a coffee cup) has the modes and Keep screen on, and its tooltip says until when. | Tray |
| <img src="assets/modules/light_switch.png" width="48" height="48" alt=""> | **Light Switch** | Switch between light and dark mode on a schedule, by shortcut, or from its own tray icon (click to switch, right-click to pick). | `Win+Ctrl+Shift+D` |
| <img src="assets/modules/quick_accent.png" width="48" height="48" alt=""> | **Quick Accent** | Hold a letter, then press Space or an arrow key to type `é`, `ñ`, `ß`… | Hold a letter |
| <img src="assets/modules/mouse_utilities.png" width="48" height="48" alt=""> | **Mouse utilities** | Find My Mouse, a click highlighter, crosshairs with a gliding cursor, and Mouse Jump. | `Ctrl` ×2, `Win+Shift+H`, `Win+Alt+P`, `Win+Shift+D` |
| <img src="assets/modules/crop_and_lock.png" width="48" height="48" alt=""> | **Crop and Lock** | Crop a window into a live thumbnail, or into a smaller window that still takes input. | `Win+Ctrl+Shift+T`, `Win+Ctrl+Shift+R` |
| <img src="assets/modules/shortcut_guide.png" width="48" height="48" alt=""> | **Shortcut Guide** | Hold the Windows key to see the shortcuts for Windows, PowerKit and the focused app, with numbers on the taskbar buttons. | Hold `Win`, or `Win+Shift+/` |
| <img src="assets/modules/grab_and_move.png" width="48" height="48" alt=""> | **Grab and Move** | Hold `Win` (or `Alt`) and drag anywhere in a window to move it; drag with the right button to resize it. | `Win` + drag |
| <img src="assets/modules/window_hopper.png" width="48" height="48" alt=""> | **Window Hopper** | Switch between the windows of the focused app, including minimized ones. | ``Alt+` ``, ``Alt+Shift+` `` |
| <img src="assets/modules/hosts_editor.png" width="48" height="48" alt=""> | **Hosts File Editor** | Add, change, turn off and delete hosts file entries, with duplicates flagged. Saving asks for administrator permission and keeps a backup. | Tray menu, `powerkit --hosts` |
| <img src="assets/modules/environment_variables.png" width="48" height="48" alt=""> | **Environment Variables** | View, add, change and delete user and system variables, with unsaved changes marked. System changes ask for administrator permission; running programs are told about the change. | Tray menu, `powerkit --env` |
| <img src="assets/modules/registry_preview.png" width="48" height="48" alt=""> | **Registry Preview** | Open or type a .reg file and see the keys and values it would write, with problems shown by line. Save it, write it to the registry (administrator permission only when it touches keys outside HKEY_CURRENT_USER), or jump to a key in Registry Editor. | Tray menu, `powerkit --reg [FILE]` |
| <img src="assets/modules/new_plus.png" width="48" height="48" alt=""> | **New+** | Right-click a folder or a folder's background and choose New+ to create a file or folder from your own templates. Leading numbers order the menu and can be hidden; `$YYYY`, `$MM`, `$PARENT_FOLDER_NAME`, `%USERNAME%` and other variables can fill in the new name. | Explorer menu, `powerkit --new FOLDER` |
| <img src="assets/modules/file_locksmith.png" width="48" height="48" alt=""> | **File Locksmith** | Right-click files or folders and choose Unlock with PowerKit to see every process that has them open, including anything inside the folders. Select one and choose End task to close it. The entry can be limited to the Shift + right-click menu. | Explorer menu, `powerkit --unlock P…` |
| <img src="assets/modules/image_resizer.png" width="48" height="48" alt=""> | **Image Resizer** | Right-click pictures and choose Resize with PowerKit, then pick a saved size or type one in pixels, percent, centimeters or inches. Fit, fill (cropping the middle) or stretch; save copies named from a pattern like `%1 (%2)` or replace the originals; never enlarge, keep metadata and dates, set the JPEG quality. Pictures Windows can read but not write, such as HEIC and WebP, are saved as JPEG or another format you choose. | Explorer menu, `powerkit --resize P…` |
| <img src="assets/modules/text_extractor.png" width="48" height="48" alt=""> | **Text Extractor** | Drag a box over any part of the screen and the text in it is read by the OCR built into Windows and copied, line by line or as one line. Reads the Windows display languages, or any language you name whose OCR is installed. | `Win+Shift+T` |
| <img src="assets/modules/power_display.png" width="48" height="48" alt=""> | **Power Display** | Set the brightness, contrast and volume of external monitors from one window, with a slider for each monitor, over DDC/CI. Input source, color preset and power can be added in the settings; each change to them asks first. Built-in laptop screens are not supported. | `Win+Alt+M` |
| <img src="assets/modules/screen_ruler.png" width="48" height="48" alt=""> | **Screen Ruler** | Measure the pixels between edges on screen: the lines stop where the color changes. Turn the wheel to measure across, down, both, or a box you drag; click to copy the size. Inches, centimeters or millimeters can be shown as well. | `Win+Ctrl+Shift+M` |
| <img src="assets/modules/zoom_it.png" width="48" height="48" alt=""> | **ZoomIt** | For presenting: zoom in on the screen around the cursor with the wheel, then click to draw on it (Shift for a line, Ctrl for a box, both for an arrow; R, G, B, Y, O, P change the color, Ctrl+Z undoes, W and K give a white or black board). Also draw straight onto the screen, live zoom, and a break countdown. | `Ctrl+1` to `Ctrl+4` |
| <img src="assets/modules/workspaces.png" width="48" height="48" alt=""> | **Workspaces** | Arrange your windows, give them a name and click Capture; later click Launch (or run `powerkit --workspace NAME`) and the programs start again with every window back on its monitor, size and maximized or minimized. Windows of programs already open are moved instead. | `Win+Ctrl+Backtick` |
| <img src="assets/modules/peek.png" width="48" height="48" alt=""> | **Peek** | Select a file in File Explorer or on the desktop and press the shortcut to see it without opening it: pictures, text and code, or a file's or folder's details. The arrow keys browse the folder (or just the selected files), Enter opens the file and Escape closes. The shortcut is only taken while File Explorer is in front. | `Ctrl+Space` |
| <img src="assets/modules/power_plan.png" width="48" height="48" alt=""> | **Power Plan Switcher** | Press the shortcut, click its own tray icon (or pick Power plan in the tray menu) for a menu of the Windows power plans at the cursor, with the power mode (best power efficiency, balanced, best performance) while Balanced is on; click one to switch. An optional second shortcut steps to the next plan. | `Win+Ctrl+Alt+P` |
| <img src="assets/modules/advanced_paste.png" width="48" height="48" alt=""> | **Advanced Paste** | Press the shortcut for a menu at the cursor of what the clipboard can be pasted as: plain text, Markdown (from formatted text), JSON (from JSON or a CSV or tab-separated table), the text in a picture, or a .txt, .html or .png file to paste into File Explorer. Plain text also has its own shortcut. | `Win+Shift+V`, `Win+Ctrl+Alt+V` |
| <img src="assets/modules/command_not_found.png" width="48" height="48" alt=""> | **Command Not Found** | Off until you turn it on. Type a command PowerShell cannot find and it shows the `winget install` line for the package that provides it. Turning it on adds a marked block to your Windows PowerShell and PowerShell 7 profiles (only those whose execution policy runs profiles); turning it off takes the block out. | — |
| <img src="assets/modules/file_explorer.png" width="48" height="48" alt=""> | **File Explorer add-ons** | In the preview pane (`Alt+P` in File Explorer): Markdown formatted with clickable links, source code in color, SVG and QOI pictures, the picture of the print in G-code and, when turned on, PDF documents. As thumbnails: SVG, QOI, G-code, STL models and, when turned on, PDF. Each can be turned off by itself; turning one off brings back what Windows showed before. They keep working while PowerKit is closed. | File Explorer |

Every shortcut can be changed or removed in the configuration, and each utility can be
turned on or off from the tray icon.

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

### Windows 11 context menu

PowerRename, Image Resizer, New+, File Locksmith and Registry Preview add their entries under **Show
more options** in Explorer's right-click menu. To put them at the top of the menu instead,
turn on **Show in the Windows 11 context menu** under General (`general.explorer_menu`).

Windows only shows entries there for a signed app package, so PowerKit builds and signs a
small one itself with a certificate it creates on your PC. The first time, Windows asks for
administrator approval so the PC trusts that certificate. Turning the setting off removes
the package and puts the entries back under Show more options; the certificate stays
trusted, so turning it on again needs no approval.

## Configuration

Choose **Open settings** in the tray menu (or click the tray icon). The Dashboard turns
each utility on or off; pick one on the left, grouped by category, to see a short animated
walkthrough of it, using your own shortcuts, above its settings. Settings use switches,
drop-downs, color pickers and a shortcut recorder (click the pencil, press the keys). Text
and number boxes save when you press Enter or leave them. Changes apply at once. The window
follows the Windows light or dark theme, or the one chosen under General.

Settings are stored in `%APPDATA%\PowerKit\config.json` (or `data\config.json` in portable
mode). PowerKit reads the file when it starts, so quit PowerKit before editing it by hand.
Invalid values are repaired, with a warning in the log, and the last good file is kept as
`config.backup.json`.

## Command line

```text
powerkit               start, or open settings if it is already running
powerkit --settings    start and open settings
powerkit --rename P…   open PowerRename with these files and folders
powerkit --hosts       open the Hosts File Editor
powerkit --env         open Environment Variables
powerkit --reg [F]     open Registry Preview, with the .reg file F
powerkit --new DIR     show the New+ template menu for folder DIR
powerkit --unlock P…   show which processes are using files or folders P
powerkit --resize P…   resize the pictures P
powerkit --trust-menu-cert HEX  trust the context menu certificate (run elevated by PowerKit)
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
