# SnipXTool

A fast, native Windows screenshot + screen recorder + image editor in one window.
Built in Rust with [egui](https://github.com/emilk/egui), so it starts instantly and
has no Electron/WebView runtime.

## Features

- **Snips** – rectangle, window, full screen, or freeform selection
- **Editor** – pen, line, arrow, rectangle, ellipse, highlight, blur/pixelate, text,
  numbered step badges, and crop; icon toolbar with one-click color swatches;
  copy or save as PNG/JPG
- **Recording** – rectangle, window, or full-screen capture to MP4 (H.264),
  microphone or system-audio loopback, countdown, and a floating recording bar
- **Video tools** – trim, mute, export a clip, or export a GIF; grab a frame into
  the image editor
- **Recent captures** list, auto-save, auto-copy to clipboard, capture delay
- **Print Screen** works anywhere as a global snip hotkey, even when the window is hidden
- **Runs in the background** – lives in the system tray, ready to snip
- **One instance only** – starting it again focuses the window that is already running
- **Start with Windows** – one switch in Settings; the autostart entry runs in the tray
- **ffmpeg bundled** – recording works right after installing, no extra setup
- Drag and drop an image/video onto the window, or open one from the command line

## Install

1. Download `SnipXTool-<version>-Setup.exe` from the [Releases](../../releases) page
   (or from the local `dist` folder after building).
2. Run it. The installer is **per-user** and does not need administrator rights.
3. Launch **SnipXTool** from the Start menu.

The first launch opens the main window so you can find your way around. After
that SnipXTool starts hidden in the system tray and keeps the Print Screen hotkey
active. Click the tray icon (or use its right-click menu) to open it again.

Optional installer task:

- Create a desktop shortcut

Everything else is configured inside the app under **⚙ Settings → Background**
(including **Start with Windows**); no installer checkbox needed.

Uninstall from **Settings → Apps → Installed apps → SnipXTool**. Uninstalling also
removes the autostart entry.

> The installer is not code-signed, so Windows SmartScreen may show
> "Windows protected your PC". Choose **More info → Run anyway**.

## Background mode and the tray

SnipXTool keeps running in the notification area by default:

- **Print Screen** starts a snip from anywhere, even with no window on screen.
- **Closing the window** hides SnipXTool to the tray instead of quitting.
- **Tray menu**: *Open SnipXTool*, *Take a snip*, *Settings…*, *Quit SnipXTool*.
- **Quit** from the tray menu is the way to fully exit.

Both behaviours can be changed in **Settings → Background**, together with the
**⚙ Start with Windows** switch, which adds or removes the autostart entry
(`HKCU\...\Run`, starting hidden in the tray). If the app was moved or
reinstalled, the entry is refreshed automatically on the next launch.

SnipXTool runs **one time only**: launching it again while it is already running
shows the existing window instead of starting a second copy.

If the tray icon cannot be created, SnipXTool keeps its window open instead of
hiding with no way to get back.

### Command-line switches

| Switch | Effect |
| --- | --- |
| `--show` | Open the main window (used by Start menu/desktop shortcuts) |
| `--background`, `--tray` | Start hidden in the tray (used by the autostart entry) |
| `<path-to-image-or-video>` | Open that file, and show the window |

## Portable version

Extract `SnipXTool-<version>-portable.zip` anywhere and run `snipxtool.exe`.
Everything is self-contained; settings and captures are still written to your
user folders (see below).

## Recording: ffmpeg is bundled

The installer and the portable zip ship with `ffmpeg.exe`, so **screen recording
and video export work right after installing** — no extra setup. (ffmpeg is a
separate GPL project; its license note is installed next to it.)

If you prefer your own ffmpeg, point SnipXTool at it in
**Settings → Recording → ffmpeg path**, or drop an `ffmpeg.exe` next to
`snipxtool.exe` — a local copy always wins. If ffmpeg is missing (for example a
`-NoFfmpeg` build), install it once with:

```powershell
winget install Gyan.FFmpeg
```

Snips and the image editor never need ffmpeg.

## Where files go

| What | Location |
| --- | --- |
| Captures | `%USERPROFILE%\Pictures\SnipXTool` (configurable in Settings; the old `Pictures\RustSnip` folder keeps working) |
| Settings | `%APPDATA%\snipxtool\settings.json` (migrated once from the old `%APPDATA%\rustsnip`) |

## Global hotkey

**Print Screen** starts a new snip from anywhere. If Windows has claimed the key,
SnipXTool explains how to free it: turn off **Settings → Accessibility → Keyboard →
"Use the Print screen button to open screen capture"**, then restart the app.

## Build from source

Prerequisites: Windows 10/11, Rust (MSVC toolchain), and the MSVC linker
(Visual Studio Build Tools with "Desktop development with C++").

```powershell
# one-time setup: Rust, ffmpeg, Inno Setup (add -InstallBuildTools if needed)
powershell -ExecutionPolicy Bypass -File scripts\setup-dev.ps1

# run from source
cargo run --release

# build the portable zip + installer into dist\
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1
```

Installer builds need [Inno Setup 6](https://jrsoftware.org/isinfo.php) (installed
by `setup-dev.ps1`). Use `-SkipInstaller` to produce only the portable zip.

By default the build **bundles ffmpeg** so users don't have to install anything.
Pass `-NoFfmpeg` for a much smaller build without it (recording then needs the
user's own ffmpeg).

## Project layout

```
SnipXTool/
  src/
    main.rs        app shell, capture flow, selection overlay, settings UI
    capture.rs     screen capture, recording, ffmpeg helpers, audio
    editor.rs      image annotation/crop editor
    video.rs       video trim/export UI
    platform.rs    capture exclusion, Print Screen hotkey, single instance, autostart
    settings.rs    persisted settings
    theme.rs       fonts and colors
    tray.rs        system tray icon and menu (background mode)
  assets/
    logo.png       window icon source
    icon.ico       embedded executable/installer icon
  installer/
    SnipXTool.iss      Inno Setup installer definition
  scripts/
    setup-dev.ps1  installs the build prerequisites
    build-release.ps1  builds the exe, portable zip, and installer
```

## Troubleshooting

- **"ffmpeg NOT found"** – install ffmpeg (above) or point Settings at `ffmpeg.exe`.
- **A second launch doesn't open a new window** – by design: SnipXTool runs once,
  and the second start just focuses the window that is already running.
- **Print Screen opens Windows Snipping Tool instead** – disable the Windows
  keyboard setting described under *Global hotkey*.
- **Recording has no audio** – pick the right source in Settings and check
  Windows microphone privacy permissions; system audio requires the loopback option.
- **SmartScreen warning** – expected for an unsigned build; use *More info → Run anyway*.
