//! Windows-only helper: hides our own window from screen capture, so the small
//! recording bar never shows up in the video (like the Windows 11 Snipping Tool).
//! On other systems these are no-ops.

#[cfg(windows)]
pub fn hwnd(frame: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

#[cfg(windows)]
pub fn exclude_from_capture(hwnd: isize, on: bool) {
    #[link(name = "user32")]
    extern "system" {
        fn SetWindowDisplayAffinity(hwnd: isize, affinity: u32) -> i32;
    }
    // WDA_EXCLUDEFROMCAPTURE = 0x11 (Windows 10 2004+), WDA_NONE = 0
    let _ = unsafe { SetWindowDisplayAffinity(hwnd, if on { 0x11 } else { 0 }) };
}

#[cfg(not(windows))]
pub fn hwnd(_frame: &eframe::Frame) -> Option<isize> {
    None
}

#[cfg(not(windows))]
pub fn exclude_from_capture(_hwnd: isize, _on: bool) {}

// ───────────── global Print Screen hotkey ─────────────

#[allow(dead_code)]
pub enum HotkeyEvent {
    Pressed { was_minimized: bool },
    /// A second launch asked the running instance to show itself.
    Activated,
    Failed(String),
}

/// Registers Print Screen as a system-wide hotkey on its own thread
/// (RegisterHotKey + message loop, no extra crates). If our window is minimized it
/// is restored first so the UI loop wakes up. The loop also watches the
/// "activate" event so a second launch can wake this instance.
#[cfg(windows)]
pub fn start_hotkey(hwnd: Option<isize>, on_event: impl Fn(HotkeyEvent) + Send + 'static) {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[repr(C)]
    struct WinMsg {
        hwnd: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt: Point,
    }
    #[link(name = "user32")]
    extern "system" {
        fn RegisterHotKey(hwnd: isize, id: i32, modifiers: u32, vk: u32) -> i32;
        fn PeekMessageW(msg: *mut WinMsg, hwnd: isize, min: u32, max: u32, remove: u32) -> i32;
        fn IsIconic(hwnd: isize) -> i32;
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
        fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
    }
    std::thread::spawn(move || unsafe {
        const MOD_NOREPEAT: u32 = 0x4000;
        const VK_SNAPSHOT: u32 = 0x2C; // the "Print Screen" / "Impr. ecran" key
        const WM_HOTKEY: u32 = 0x0312;
        const SW_SHOW: i32 = 5;
        const SW_RESTORE: i32 = 9;
        const PM_REMOVE: u32 = 1;
        const EVENT_MODIFY_STATE: u32 = 0x0002;
        const SYNCHRONIZE: u32 = 0x0010_0000;
        if RegisterHotKey(0, 1, MOD_NOREPEAT, VK_SNAPSHOT) == 0 {
            on_event(HotkeyEvent::Failed(
                "Print Screen is already used by Windows. Turn off Settings > Accessibility > Keyboard > \
                 \"Use the Print screen button to open screen capture\", then restart SnipXTool."
                    .to_string(),
            ));
            return;
        }
        // Wake-up channel used by the single-instance guard (second launches).
        let mut name: Vec<u16> = ACTIVATE_EVENT.encode_utf16().collect();
        name.push(0);
        let activate = OpenEventW(EVENT_MODIFY_STATE | SYNCHRONIZE, 0, name.as_ptr());
        let wake = |h: isize, minimized: &mut bool| {
            if IsIconic(h) != 0 {
                *minimized = true;
                ShowWindow(h, SW_RESTORE);
            } else {
                // Hidden windows do not receive repaint requests; show them so
                // the UI loop wakes up and handles the event.
                ShowWindow(h, SW_SHOW);
            }
            SetForegroundWindow(h);
        };
        let mut msg = WinMsg { hwnd: 0, message: 0, wparam: 0, lparam: 0, time: 0, pt: Point { x: 0, y: 0 } };
        loop {
            while PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) > 0 {
                if msg.message == WM_HOTKEY {
                    let mut was_minimized = false;
                    if let Some(h) = hwnd {
                        wake(h, &mut was_minimized);
                    }
                    on_event(HotkeyEvent::Pressed { was_minimized });
                }
            }
            if activate != 0 && WaitForSingleObject(activate, 60) == 0 {
                let mut was_minimized = false;
                if let Some(h) = hwnd {
                    wake(h, &mut was_minimized);
                }
                on_event(HotkeyEvent::Activated);
            }
        }
    });
}

#[cfg(not(windows))]
pub fn start_hotkey(_hwnd: Option<isize>, _on_event: impl Fn(HotkeyEvent) + Send + 'static) {}

// ───────────── single instance ─────────────

/// Auto-reset event used to wake the running instance when the app is started
/// again ("open one time only": the second process exits after signalling).
#[cfg(windows)]
pub const ACTIVATE_EVENT: &str = "Local\\SnipXTool.Activate";

/// Returns `true` when this is the first instance (and owns the event).
/// Returns `false` when another instance is already running.
#[cfg(windows)]
pub fn acquire_single_instance() -> bool {
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateEventW(attrs: *mut u8, manual: i32, initial: i32, name: *const u16) -> isize;
        fn GetLastError() -> u32;
        fn CloseHandle(handle: isize) -> i32;
    }
    const ERROR_ALREADY_EXISTS: u32 = 183;
    let mut name: Vec<u16> = ACTIVATE_EVENT.encode_utf16().collect();
    name.push(0);
    unsafe {
        let h = CreateEventW(std::ptr::null_mut(), 0, 0, name.as_ptr());
        if h == 0 {
            return true; // cannot enforce; do not block the app
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(h);
            return false;
        }
        // Deliberately never close the event: it must stay alive for the whole
        // process lifetime so later launches can find it.
        let _ = h;
    }
    true
}

/// Wakes the already-running instance (shows its window).
#[cfg(windows)]
pub fn activate_existing() {
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
        fn SetEvent(handle: isize) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }
    const EVENT_MODIFY_STATE: u32 = 0x0002;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    let mut name: Vec<u16> = ACTIVATE_EVENT.encode_utf16().collect();
    name.push(0);
    unsafe {
        let h = OpenEventW(EVENT_MODIFY_STATE | SYNCHRONIZE, 0, name.as_ptr());
        if h != 0 {
            SetEvent(h);
            CloseHandle(h);
        }
    }
}

#[cfg(not(windows))]
pub fn acquire_single_instance() -> bool {
    true
}

#[cfg(not(windows))]
pub fn activate_existing() {}

// ───────────── start with Windows (autostart) ─────────────

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "SnipXTool";

/// Whether the HKCU Run entry for SnipXTool currently exists.
#[cfg(windows)]
pub fn autostart_enabled() -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("reg")
        .args(["query", RUN_KEY, "/v", RUN_VALUE])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Writes or removes the HKCU Run entry (points at this exe with --background).
/// Calling it again with `true` also refreshes the stored path after the app
/// was moved or reinstalled somewhere else.
#[cfg(windows)]
pub fn autostart_set(enable: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate the app: {e}"))?;
    let data = format!("\"{}\" --background", exe.display());
    let result = if enable {
        std::process::Command::new("reg")
            .args(["add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &data, "/f"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    } else {
        std::process::Command::new("reg")
            .args(["delete", RUN_KEY, "/v", RUN_VALUE, "/f"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    };
    match result {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(not(windows))]
pub fn autostart_enabled() -> bool {
    false
}

/// Removes autostart entries written by previous builds of the app ("SnipX"
/// and the original "RustSnip"), if any.
#[cfg(windows)]
pub fn autostart_remove_legacy() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    for legacy in ["SnipX", "RustSnip"] {
        let _ = std::process::Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                legacy,
                "/f",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
}

/// The current Run entry value, if it exists (reg query output line after REG_SZ).
#[cfg(windows)]
fn autostart_data() -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("reg")
        .args(["query", RUN_KEY, "/v", RUN_VALUE])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find(|l| l.contains("REG_SZ"))
        .and_then(|l| l.split("REG_SZ").nth(1))
        .map(|s| s.trim().to_string())
}

/// True when the Run entry points at this exact executable with --background.
/// Used to refresh the entry when the exe was moved or renamed.
#[cfg(windows)]
pub fn autostart_matches() -> bool {
    let Some(data) = autostart_data() else { return false };
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return false,
    };
    data.contains(&format!("\"{}\"", exe.display())) && data.ends_with("--background")
}

#[cfg(not(windows))]
pub fn autostart_set(_enable: bool) -> Result<(), String> {
    Err("autostart is Windows-only".to_string())
}
