#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod capture;
mod editor;
mod platform;
mod settings;
mod theme;
mod tray;
mod video;

use editor::{Action, Editor};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Stroke, Vec2};
use image::RgbaImage;
use settings::{ImgFormat, Settings, LOOPBACK, MIC_DEFAULT};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};
use video::{VideoAction, VideoEditor};

/// Size of the small floating bar shown while recording.
const PILL: Vec2 = Vec2::new(310.0, 54.0);

/// Results coming back from worker threads.
enum Msg {
    Recorded(capture::RecordResult),
    VideoLoaded(PathBuf, f64),
    /// (frame, open it in the image editor?)
    Frame(Result<RgbaImage, String>, bool),
    Exported(Result<PathBuf, String>),
    /// Print Screen was pressed
    Hotkey { was_minimized: bool },
    /// A second launch asked this instance to show itself
    Activated,
    HotkeyFailed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Home,
    Image,
    Video,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Snip,
    Record,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Rectangle,
    Window,
    FullScreen,
    Freeform,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Rectangle => "Rectangle",
            Mode::Window => "Window",
            Mode::FullScreen => "Full screen",
            Mode::Freeform => "Freeform",
        }
    }
}

fn modes_for(tab: Tab) -> Vec<Mode> {
    if tab == Tab::Snip {
        vec![Mode::Rectangle, Mode::Window, Mode::FullScreen, Mode::Freeform]
    } else {
        vec![Mode::Rectangle, Mode::Window, Mode::FullScreen]
    }
}

#[derive(Clone, Copy)]
struct Region {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// The dimmed full-screen overlay where you pick what to capture.
struct Selection {
    tab: Tab,
    mode: Mode,
    shot: RgbaImage,
    tex: egui::TextureHandle,
    windows: Vec<[f32; 4]>,     // screenshot pixels
    drag: Option<(Pos2, Pos2)>, // screen points
    free: Vec<Pos2>,            // screen points
    region: Option<Rect>,       // screenshot pixels (Record: chosen, waiting for Start)
}

enum Phase {
    Idle,
    /// our window is moved off-screen; capture when `at` is reached
    Hiding { at: Instant, tab: Tab, mode: Mode },
    Selecting(Box<Selection>),
    Countdown { until: Instant, region: Option<Region> },
    Recording,
}

enum Outcome {
    None,
    Cancel,
    Crop(Rect),
    Free(Vec<Pos2>),
    Full,
    Start(Rect),
}

struct App {
    cfg: Settings,
    view: View,
    tab: Tab,
    mode: Mode,
    phase: Phase,
    editor: Option<Editor>,
    video: Option<VideoEditor>,
    history: Vec<PathBuf>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    status: String,
    recorder: Option<capture::Recorder>,
    show_settings: bool,
    audio_devices: Vec<String>,
    font: Option<ab_glyph::FontArc>,
    ffmpeg_ok: bool,
    clipboard: Option<arboard::Clipboard>,
    saved_geom: Option<(Pos2, Vec2)>,     // window position/size before capturing
    geom: Option<(Pos2, Vec2, u32)>,      // geometry to (re)apply for a few frames
    cap_mon: capture::MonInfo,
    hwnd: Option<isize>,
    hotkey_started: bool,
    autostart_synced: bool,
    tray: Option<tray::Tray>,
    hidden: bool,
    quitting: bool,
}

fn main() -> eframe::Result<()> {
    // "Open one time only": a second launch wakes the running instance instead.
    if !platform::acquire_single_instance() {
        platform::activate_existing();
        return Ok(());
    }
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let initial = args.iter().skip(1).map(PathBuf::from).find(|p| p.is_file());
    let force_show = args.iter().any(|a| a.to_string_lossy() == "--show");
    let force_background = args
        .iter()
        .any(|a| matches!(a.to_string_lossy().as_ref(), "--background" | "--tray"));

    let cfg = Settings::load();
    // First launch shows the window so the app is discoverable; after that it
    // starts in the tray by default. Explicit shortcuts pass --show/--background.
    let first_run = !Settings::exists();
    if first_run {
        // Write defaults now so the *next* launch starts in the background.
        cfg.save();
    }
    let start_hidden = initial.is_none()
        && !force_show
        && (force_background || (!first_run && cfg.start_in_background));

    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SnipXTool")
            .with_icon(load_icon())
            .with_inner_size([1100.0, 740.0])
            .with_min_inner_size([760.0, 480.0])
            .with_visible(!start_hidden),
        ..Default::default()
    };
    eframe::run_native(
        "SnipXTool",
        opts,
        Box::new(move |cc| Ok(Box::new(App::new(cc, cfg, start_hidden, initial)))),
    )
}

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        cfg: Settings,
        start_hidden: bool,
        initial: Option<PathBuf>,
    ) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let (tx, rx) = channel();
        let ffmpeg_ok = capture::ffmpeg_available(&cfg.ffmpeg_path);
        let history = scan_dir(&cfg.save_dir);
        let font = editor::load_font(&cfg.font_path);
        let mut app = Self {
            cfg,
            view: View::Home,
            tab: Tab::Snip,
            mode: Mode::Rectangle,
            phase: Phase::Idle,
            editor: None,
            video: None,
            history,
            tx,
            rx,
            status: "Ready".into(),
            recorder: None,
            show_settings: false,
            audio_devices: capture::list_audio_inputs(),
            font,
            ffmpeg_ok,
            clipboard: arboard::Clipboard::new().ok(),
            saved_geom: None,
            geom: None,
            cap_mon: capture::MonInfo::default(),
            hwnd: None,
            hotkey_started: false,
            autostart_synced: false,
            tray: None,
            hidden: start_hidden,
            quitting: false,
        };

        app.tray = tray::Tray::new(cc.egui_ctx.clone());
        if start_hidden && app.tray.is_none() {
            // Never leave the app running with no window and no way to reach it.
            app.hidden = false;
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::Visible(true));
            app.status = "Tray icon unavailable - keeping the window open".into();
        } else if app.hidden {
            app.status =
                "Running in the background - right-click the tray icon or press Print Screen".into();
        }

        // Open a file passed on the command line (e.g. "Open with" from Explorer).
        if let Some(path) = initial {
            app.open_path(&cc.egui_ctx, path);
        }
        app
    }

    // ───────────── helpers ─────────────

    fn show_window(&mut self, ctx: &egui::Context) {
        self.hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        if self.tray.is_none() {
            return;
        }
        self.hidden = true;
        self.status = "Running in the background - press Print Screen to snip".into();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.cfg.save();
        self.quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Keeps the Windows "Run" registry entry in step with `cfg.autostart`,
    /// also refreshing the stored exe path after a move or reinstall.
    fn sync_autostart(&mut self) {
        #[cfg(windows)]
        {
            let needs_write = if self.cfg.autostart {
                !platform::autostart_enabled() || !platform::autostart_matches()
            } else {
                platform::autostart_enabled()
            };
            if needs_write {
                if let Err(e) = platform::autostart_set(self.cfg.autostart) {
                    self.status = format!("Autostart not updated: {e}");
                } else if self.cfg.autostart {
                    self.status = "SnipXTool will start with Windows (in the tray)".into();
                }
            }
        }
    }

    fn new_path(&self, prefix: &str, ext: &str) -> PathBuf {
        let _ = std::fs::create_dir_all(&self.cfg.save_dir);
        let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        self.cfg.save_dir.join(format!("{prefix}_{stamp}.{ext}"))
    }

    fn refresh_history(&mut self) {
        self.history = scan_dir(&self.cfg.save_dir);
    }

    fn copy_image(&mut self, img: &RgbaImage) {
        let Some(cb) = self.clipboard.as_mut() else {
            self.status = "Clipboard unavailable".into();
            return;
        };
        let data = arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: std::borrow::Cow::Borrowed(img.as_raw().as_slice()),
        };
        self.status = match cb.set_image(data) {
            Ok(_) => "Snip copied to clipboard".into(),
            Err(e) => format!("Clipboard error: {e}"),
        };
    }

    /// Puts the window back to its normal size/position/decorations.
    fn restore_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        if let Some((pos, size)) = self.saved_geom {
            self.geom = Some((pos, size, 25));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Window manager commands right after leaving fullscreen can be ignored,
    /// so geometry is re-sent for a few frames.
    fn apply_geom(&mut self, ctx: &egui::Context) {
        if let Some((pos, size, n)) = self.geom {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
            self.geom = if n > 1 { Some((pos, size, n - 1)) } else { None };
            ctx.request_repaint();
        }
    }

    // ───────────── starting a capture ─────────────

    fn new_clicked(&mut self, ctx: &egui::Context, extra_hide: Duration) {
        if self.hidden {
            self.show_window(ctx);
        }
        let (tab, mode) = (self.tab, self.mode);
        if tab == Tab::Record && !self.ffmpeg_ok {
            self.status = "ffmpeg not found - install it or set its path in Settings".into();
            self.show_settings = true;
            return;
        }
        let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
        // ignore bogus geometry reported while the window is minimized / off-screen
        let fresh = match (outer, inner) {
            (Some(o), Some(i))
                if o.min.x > -10000.0 && o.min.y > -10000.0 && i.width() > 100.0 && i.height() > 100.0 =>
            {
                Some((o.min, i.size()))
            }
            _ => None,
        };
        self.saved_geom = fresh
            .or(self.saved_geom)
            .or(Some((Pos2::new(100.0, 100.0), Vec2::new(1100.0, 720.0))));
        let ppp = ctx.pixels_per_point();
        self.cap_mon = capture::monitor_at(outer.map(|o| (o.center().x * ppp, o.center().y * ppp)));

        if tab == Tab::Record && mode == Mode::FullScreen {
            self.start_countdown(ctx, None);
            return;
        }
        // move our window out of the way, then grab the screen
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(-32000.0, -32000.0)));
        let delay = if tab == Tab::Snip {
            Duration::from_secs(self.cfg.delay_secs as u64)
        } else {
            Duration::ZERO
        };
        self.phase = Phase::Hiding {
            at: Instant::now() + Duration::from_millis(250) + delay + extra_hide,
            tab,
            mode,
        };
        self.status = "Capturing...".into();
    }

    fn drive(&mut self, ctx: &egui::Context) {
        if let Phase::Hiding { at, tab, mode } = self.phase {
            if Instant::now() < at {
                ctx.request_repaint_after(Duration::from_millis(30));
                return;
            }
            self.phase = Phase::Idle;
            match capture::grab_screen(self.cap_mon.idx) {
                Err(e) => {
                    self.restore_window(ctx);
                    self.status = format!("Capture failed: {e}");
                }
                Ok(shot) => {
                    if mode == Mode::FullScreen {
                        self.restore_window(ctx);
                        self.finish_snip(shot);
                    } else {
                        self.begin_selection(ctx, tab, mode, shot);
                    }
                }
            }
        }
    }

    fn begin_selection(&mut self, ctx: &egui::Context, tab: Tab, mode: Mode, shot: RgbaImage) {
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [shot.width() as usize, shot.height() as usize],
            shot.as_raw(),
        );
        let tex = ctx.load_texture("shot", ci, egui::TextureOptions::LINEAR);
        let windows = capture::list_windows(self.cap_mon);
        self.phase = Phase::Selecting(Box::new(Selection {
            tab,
            mode,
            shot,
            tex,
            windows,
            drag: None,
            free: Vec::new(),
            region: None,
        }));
        // our window becomes the dimmed overlay
        if let Some((pos, _)) = self.saved_geom {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn finish_snip(&mut self, img: RgbaImage) {
        if self.cfg.auto_copy {
            self.copy_image(&img);
        }
        let mut saved = None;
        let mut note = String::from("Snip captured");
        if self.cfg.auto_save {
            let p = self.new_path("snip", self.cfg.format.ext());
            match editor::save_image(&img, &p, self.cfg.jpg_quality) {
                Ok(()) => {
                    note = format!("Snip saved to {}", p.display());
                    saved = Some(p);
                }
                Err(e) => note = format!("Auto-save failed: {e}"),
            }
            self.refresh_history();
        }
        let mut ed = Editor::new(img, &self.cfg);
        ed.path = saved;
        self.editor = Some(ed);
        self.view = View::Image;
        self.status = note;
    }

    // ───────────── recording ─────────────

    fn start_countdown(&mut self, ctx: &egui::Context, region: Option<Region>) {
        self.phase = Phase::Countdown { until: Instant::now() + Duration::from_secs(3), region };
        // shrink to a small always-on-top bar at the top centre of the recorded screen
        let ppp = ctx.pixels_per_point();
        let m = self.cap_mon;
        let pos = Pos2::new(
            m.x as f32 / ppp + (m.w as f32 / ppp - PILL.x) / 2.0,
            m.y as f32 / ppp + 14.0,
        );
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        self.geom = Some((pos, PILL, 40));
        if let Some(h) = self.hwnd {
            platform::exclude_from_capture(h, true);
        }
    }

    fn begin_record(&mut self, ctx: &egui::Context, region: Option<Region>) {
        let out = self.new_path("rec", "mp4");
        let (tx, c) = (self.tx.clone(), ctx.clone());
        let reg = region.map(|r| (r.x, r.y, r.w, r.h));
        self.recorder = Some(capture::start_recording(
            self.cfg.clone(),
            out,
            self.cap_mon.idx,
            reg,
            Duration::from_millis(0),
            move |r| {
                let _ = tx.send(Msg::Recorded(r));
                c.request_repaint();
            },
        ));
        self.phase = Phase::Recording;
        self.status = "Recording...".into();
    }

    fn stop_recording(&mut self) {
        if let Some(r) = &self.recorder {
            r.stop.store(true, Ordering::Relaxed);
            self.status = "Finalizing video...".into();
        }
    }

    fn end_pill(&mut self, ctx: &egui::Context) {
        if let Some(h) = self.hwnd {
            platform::exclude_from_capture(h, false);
        }
        self.restore_window(ctx);
    }

    fn on_recorded(&mut self, ctx: &egui::Context, res: capture::RecordResult) {
        self.recorder = None;
        self.phase = Phase::Idle;
        self.end_pill(ctx);
        match res {
            Ok((path, warn)) => {
                self.status = match warn {
                    Some(w) => format!("Saved {} ({w})", path.display()),
                    None => format!("Saved {}", path.display()),
                };
                self.refresh_history();
                self.open_video(ctx, path);
            }
            Err(e) => self.status = format!("Recording failed: {e}"),
        }
    }

    // ───────────── opening files ─────────────

    fn open_path(&mut self, ctx: &egui::Context, p: PathBuf) {
        if is_video(&p) {
            self.open_video(ctx, p);
            return;
        }
        match image::open(&p) {
            Ok(i) => {
                let mut ed = Editor::new(i.to_rgba8(), &self.cfg);
                ed.path = Some(p);
                self.editor = Some(ed);
                self.view = View::Image;
            }
            Err(e) => self.status = format!("Cannot open image: {e}"),
        }
    }

    fn open_video(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.video = Some(VideoEditor::new(path.clone()));
        self.view = View::Video;
        let (tx, c, ff) = (self.tx.clone(), ctx.clone(), self.cfg.ffmpeg_path.clone());
        std::thread::spawn(move || {
            let dur = capture::probe_duration(&ff, &path).unwrap_or(0.0);
            let _ = tx.send(Msg::VideoLoaded(path.clone(), dur));
            let _ = tx.send(Msg::Frame(capture::grab_frame(&ff, &path, 0.0), false));
            c.request_repaint();
        });
    }

    fn handle_video_action(&mut self, ctx: &egui::Context, act: VideoAction) {
        let Some(v) = self.video.as_ref() else { return };
        let (path, pos, start, end, mute) = (v.path.clone(), v.pos, v.start, v.end, v.mute);
        let ff = self.cfg.ffmpeg_path.clone();
        match act {
            VideoAction::Play => {
                let _ = open::that(&path);
            }
            VideoAction::Preview | VideoAction::GrabFrame => {
                let to_editor = matches!(act, VideoAction::GrabFrame);
                let (tx, c) = (self.tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::Frame(capture::grab_frame(&ff, &path, pos), to_editor));
                    c.request_repaint();
                });
            }
            VideoAction::Export | VideoAction::Gif => {
                let gif = matches!(act, VideoAction::Gif);
                if let Some(v) = self.video.as_mut() {
                    v.busy = true;
                }
                let out = self.new_path(if gif { "clip" } else { "edit" }, if gif { "gif" } else { "mp4" });
                let crf = self.cfg.crf;
                let (tx, c) = (self.tx.clone(), ctx.clone());
                self.status = "Exporting...".into();
                std::thread::spawn(move || {
                    let r = capture::export_clip(&ff, &path, &out, start, end, mute, crf, gif);
                    let _ = tx.send(Msg::Exported(r));
                    c.request_repaint();
                });
            }
        }
    }

    fn handle_editor_action(&mut self, a: Action, ed: &mut Editor) {
        match a {
            Action::Copy => {
                let img = ed.flattened(&self.font);
                self.copy_image(&img);
            }
            Action::Save => {
                let ext = self.cfg.format.ext();
                let path = ed.path.clone().unwrap_or_else(|| self.new_path("snip", ext));
                self.save_to(ed, path);
            }
            Action::SaveAs => {
                let ext = self.cfg.format.ext();
                let picked = rfd::FileDialog::new()
                    .set_directory(&self.cfg.save_dir)
                    .set_file_name(format!("snip.{ext}"))
                    .add_filter("PNG", &["png"])
                    .add_filter("JPEG", &["jpg", "jpeg"])
                    .save_file();
                if let Some(p) = picked {
                    self.save_to(ed, p);
                }
            }
            Action::Close => {}
        }
    }

    fn save_to(&mut self, ed: &mut Editor, path: PathBuf) {
        let img = ed.flattened(&self.font);
        match editor::save_image(&img, &path, self.cfg.jpg_quality) {
            Ok(()) => {
                self.status = format!("Saved {}", path.display());
                ed.path = Some(path);
                self.refresh_history();
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    // ───────────── message pump ─────────────

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Recorded(res) => self.on_recorded(ctx, res),
                Msg::Hotkey { was_minimized } => {
                    if matches!(self.phase, Phase::Idle) {
                        self.show_window(ctx);
                        self.tab = Tab::Snip;
                        let extra = if was_minimized { Duration::from_millis(500) } else { Duration::ZERO };
                        self.new_clicked(ctx, extra);
                    }
                }
                Msg::Activated => self.show_window(ctx),
                Msg::HotkeyFailed(e) => self.status = e,
                Msg::VideoLoaded(path, dur) => {
                    if let Some(v) = self.video.as_mut() {
                        if v.path == path {
                            v.duration = dur;
                            v.start = 0.0;
                            v.end = dur;
                        }
                    }
                }
                Msg::Frame(res, to_editor) => match res {
                    Ok(img) => {
                        if to_editor {
                            self.editor = Some(Editor::new(img, &self.cfg));
                            self.view = View::Image;
                            self.status = "Frame grabbed - annotate it and save".into();
                        } else if let Some(v) = self.video.as_mut() {
                            let ci = egui::ColorImage::from_rgba_unmultiplied(
                                [img.width() as usize, img.height() as usize],
                                img.as_raw(),
                            );
                            v.tex = Some(ctx.load_texture("vframe", ci, egui::TextureOptions::LINEAR));
                        }
                    }
                    Err(e) => self.status = format!("Frame error: {e}"),
                },
                Msg::Exported(res) => {
                    if let Some(v) = self.video.as_mut() {
                        v.busy = false;
                    }
                    match res {
                        Ok(p) => {
                            self.status = format!("Saved {}", p.display());
                            self.refresh_history();
                        }
                        Err(e) => {
                            self.status =
                                format!("Export failed: {}", e.lines().last().unwrap_or("unknown error"))
                        }
                    }
                }
            }
        }
    }

    // ───────────── overlay: choose what to capture ─────────────

    fn selection_ui(&mut self, ctx: &egui::Context) {
        let mut outcome = Outcome::None;
        let mut new_mode: Option<Mode> = None;

        if let Phase::Selecting(sel) = &mut self.phase {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                outcome = Outcome::Cancel;
            }

            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(Color32::BLACK))
                .show(ctx, |ui| {
                    let rect = ui.max_rect();
                    let (resp, painter) = ui.allocate_painter(rect.size(), egui::Sense::click_and_drag());
                    let (iw, ih) = (sel.shot.width() as f32, sel.shot.height() as f32);
                    let (kx, ky) = (iw / rect.width(), ih / rect.height());
                    let to_px = |p: Pos2| Pos2::new((p.x - rect.min.x) * kx, (p.y - rect.min.y) * ky);
                    let to_pt = |p: Pos2| Pos2::new(rect.min.x + p.x / kx, rect.min.y + p.y / ky);

                    painter.image(
                        sel.tex.id(),
                        rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);

                    let mut active: Option<Rect> = None; // highlighted area, screen points
                    let mut done: Option<Rect> = None; // finished selection, screenshot pixels

                    match sel.mode {
                        Mode::Rectangle => {
                            if resp.drag_started() {
                                if let Some(p) = resp.interact_pointer_pos() {
                                    sel.drag = Some((p, p));
                                    sel.region = None;
                                }
                            } else if resp.dragged() {
                                if let (Some((a, _)), Some(p)) = (sel.drag, resp.interact_pointer_pos()) {
                                    sel.drag = Some((a, p));
                                }
                            }
                            if let Some((a, b)) = sel.drag {
                                active = Some(Rect::from_two_pos(a, b));
                            }
                            if resp.drag_stopped() {
                                if let Some((a, b)) = sel.drag.take() {
                                    let r = Rect::from_two_pos(to_px(a), to_px(b));
                                    if r.width() > 4.0 && r.height() > 4.0 {
                                        done = Some(r);
                                    }
                                }
                            }
                        }
                        Mode::Window => {
                            if let Some(h) = resp.hover_pos() {
                                let hp = to_px(h);
                                let hit = sel
                                    .windows
                                    .iter()
                                    .find(|w| hp.x >= w[0] && hp.x < w[2] && hp.y >= w[1] && hp.y < w[3]);
                                if let Some(w) = hit {
                                    let r_px = Rect::from_min_max(Pos2::new(w[0], w[1]), Pos2::new(w[2], w[3]));
                                    active = Some(Rect::from_min_max(to_pt(r_px.min), to_pt(r_px.max)));
                                    if resp.clicked() {
                                        done = Some(r_px);
                                    }
                                }
                            }
                        }
                        Mode::Freeform => {
                            if resp.drag_started() {
                                sel.free.clear();
                            }
                            if resp.dragged() {
                                if let Some(p) = resp.interact_pointer_pos() {
                                    if sel.free.last().map_or(true, |l| l.distance(p) > 2.0) {
                                        sel.free.push(p);
                                    }
                                }
                            }
                            if resp.drag_stopped() {
                                if sel.free.len() > 3 {
                                    outcome = Outcome::Free(sel.free.iter().map(|p| to_px(*p)).collect());
                                }
                                sel.free.clear();
                            }
                        }
                        Mode::FullScreen => {}
                    }

                    if active.is_none() {
                        if let Some(r) = sel.region {
                            active = Some(Rect::from_min_max(to_pt(r.min), to_pt(r.max)));
                        }
                    }

                    // dim everything except the active area
                    let dim = Color32::from_black_alpha(120);
                    match active {
                        Some(a) => {
                            let a = a.intersect(rect);
                            painter.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.max.x, a.min.y)), 0.0, dim);
                            painter.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, a.max.y), rect.max), 0.0, dim);
                            painter.rect_filled(
                                Rect::from_min_max(Pos2::new(rect.min.x, a.min.y), Pos2::new(a.min.x, a.max.y)),
                                0.0,
                                dim,
                            );
                            painter.rect_filled(
                                Rect::from_min_max(Pos2::new(a.max.x, a.min.y), Pos2::new(rect.max.x, a.max.y)),
                                0.0,
                                dim,
                            );
                            painter.rect_stroke(a, 0.0, Stroke::new(2.0_f32, Color32::WHITE));
                        }
                        None => {
                            painter.rect_filled(rect, 0.0, dim);
                        }
                    }
                    if sel.free.len() > 1 {
                        painter.add(egui::Shape::line(sel.free.clone(), Stroke::new(2.0_f32, Color32::WHITE)));
                    }

                    if let Some(r) = done {
                        if sel.tab == Tab::Snip {
                            outcome = Outcome::Crop(r);
                        } else {
                            sel.region = Some(r);
                        }
                    }
                });

            // floating mode bar, top centre (like the Windows 11 Snipping Tool)
            egui::Area::new(egui::Id::new("snip_toolbar"))
                .anchor(egui::Align2::CENTER_TOP, [0.0, 18.0])
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .rounding(8.0)
                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for m in modes_for(sel.tab) {
                                    if ui.selectable_label(sel.mode == m, m.label()).clicked() {
                                        new_mode = Some(m);
                                    }
                                }
                                ui.separator();
                                if ui.button("Close").clicked() {
                                    outcome = Outcome::Cancel;
                                }
                            });
                        });
                });

            if sel.tab == Tab::Record {
                egui::Area::new(egui::Id::new("rec_start"))
                    .anchor(egui::Align2::CENTER_TOP, [0.0, 74.0])
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style())
                            .rounding(8.0)
                            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                            .show(ui, |ui| match sel.region {
                                Some(r) => {
                                    let btn = egui::Button::new(RichText::new("Start recording").color(Color32::WHITE))
                                        .fill(theme::ACCENT);
                                    if ui.add(btn).clicked() {
                                        outcome = Outcome::Start(r);
                                    }
                                }
                                None => {
                                    ui.label("Select the area you want to record");
                                }
                            });
                    });
            }
        }

        if let Some(m) = new_mode {
            if let Phase::Selecting(sel) = &mut self.phase {
                sel.mode = m;
                sel.drag = None;
                sel.free.clear();
                sel.region = None;
            }
            self.mode = m;
            if m == Mode::FullScreen {
                outcome = Outcome::Full;
            }
        }

        if !matches!(outcome, Outcome::None) {
            let Phase::Selecting(sel) = std::mem::replace(&mut self.phase, Phase::Idle) else {
                return;
            };
            let Selection { tab, shot, .. } = *sel;
            match outcome {
                Outcome::None | Outcome::Cancel => {
                    self.restore_window(ctx);
                    self.status = "Capture cancelled".into();
                }
                Outcome::Crop(r) => {
                    let img = capture::crop(&shot, [r.min.x, r.min.y, r.max.x, r.max.y]);
                    self.restore_window(ctx);
                    self.finish_snip(img);
                }
                Outcome::Free(pts) => {
                    let p: Vec<(f32, f32)> = pts.iter().map(|q| (q.x, q.y)).collect();
                    self.restore_window(ctx);
                    match capture::freeform_cut(&shot, &p) {
                        Some(img) => self.finish_snip(img),
                        None => self.status = "Selection too small".into(),
                    }
                }
                Outcome::Full => {
                    if tab == Tab::Snip {
                        self.restore_window(ctx);
                        self.finish_snip(shot);
                    } else {
                        self.start_countdown(ctx, None);
                    }
                }
                Outcome::Start(r) => {
                    let reg = region_from(r, shot.width(), shot.height());
                    self.start_countdown(ctx, reg);
                }
            }
        }
    }

    // ───────────── recording bar ─────────────

    fn pill_ui(&mut self, ctx: &egui::Context) {
        ctx.request_repaint_after(Duration::from_millis(100));
        if let Phase::Countdown { until, region } = self.phase {
            if Instant::now() >= until {
                self.begin_record(ctx, region);
            }
        }
        let countdown = if let Phase::Countdown { until, .. } = self.phase {
            Some(until.saturating_duration_since(Instant::now()).as_secs() + 1)
        } else {
            None
        };
        let rec = self
            .recorder
            .as_ref()
            .map(|r| (r.started.elapsed().as_secs(), r.stop.load(Ordering::Relaxed)));
        let mut stop = false;
        let mut cancel = false;

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(32, 32, 32))
                    .inner_margin(egui::Margin::symmetric(14.0, 10.0)),
            )
            .show(ctx, |ui| {
                let drag = ui.interact(ui.max_rect(), egui::Id::new("pill_drag"), egui::Sense::drag());
                if drag.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                ui.horizontal_centered(|ui| {
                    if let Some(n) = countdown {
                        ui.label(RichText::new(format!("Recording starts in {n}")).color(Color32::WHITE));
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    } else if let Some((secs, stopping)) = rec {
                        ui.label(RichText::new("REC").color(Color32::from_rgb(230, 60, 60)).strong());
                        ui.label(
                            RichText::new(format!("{:02}:{:02}", secs / 60, secs % 60))
                                .color(Color32::WHITE)
                                .monospace(),
                        );
                        let label = if stopping { "Saving..." } else { "Stop" };
                        let btn = egui::Button::new(RichText::new(label).color(Color32::WHITE))
                            .fill(Color32::from_rgb(190, 40, 40));
                        if ui.add_enabled(!stopping, btn).clicked() {
                            stop = true;
                        }
                    }
                });
            });

        if cancel {
            self.phase = Phase::Idle;
            self.end_pill(ctx);
            self.status = "Recording cancelled".into();
        }
        if stop {
            self.stop_recording();
        }
    }

    // ───────────── normal window UI ─────────────

    fn top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(6.0);
        let delay_before = self.cfg.delay_secs;
        ui.horizontal(|ui| {
            if ui.selectable_label(self.tab == Tab::Snip, "📷 Snip").clicked() {
                self.tab = Tab::Snip;
            }
            if ui.selectable_label(self.tab == Tab::Record, "🎬 Record").clicked() {
                self.tab = Tab::Record;
                if self.mode == Mode::Freeform {
                    self.mode = Mode::Rectangle;
                }
            }
            ui.separator();

            let busy = !matches!(self.phase, Phase::Idle);
            let new_btn =
                egui::Button::new(RichText::new("📷 New").color(Color32::WHITE)).fill(theme::ACCENT);
            if ui.add_enabled(!busy, new_btn).clicked() {
                self.new_clicked(ctx, Duration::ZERO);
            }

            egui::ComboBox::from_id_salt("mode")
                .selected_text(self.mode.label())
                .show_ui(ui, |ui| {
                    for m in modes_for(self.tab) {
                        ui.selectable_value(&mut self.mode, m, m.label());
                    }
                });

            if self.tab == Tab::Snip {
                let label = if self.cfg.delay_secs == 0 {
                    "No delay".to_string()
                } else {
                    format!("{} s delay", self.cfg.delay_secs)
                };
                egui::ComboBox::from_id_salt("delay").selected_text(label).show_ui(ui, |ui| {
                    for d in [0u32, 3, 5, 10] {
                        let text = if d == 0 { "No delay".to_string() } else { format!("{d} seconds") };
                        ui.selectable_value(&mut self.cfg.delay_secs, d, text);
                    }
                });
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .selectable_label(self.show_settings, "⚙ Settings")
                    .on_hover_text("Settings")
                    .clicked()
                {
                    self.show_settings = !self.show_settings;
                }
                if self.tray.is_some()
                    && ui
                        .button("⬇ Hide")
                        .on_hover_text("Hide to tray - Print Screen keeps working")
                        .clicked()
                {
                    self.hide_to_tray(ctx);
                }
                if ui.button("📁 Open").on_hover_text("Open a capture").clicked() {
                    let picked = rfd::FileDialog::new()
                        .add_filter("Captures", &["png", "jpg", "jpeg", "mp4"])
                        .set_directory(&self.cfg.save_dir)
                        .pick_file();
                    if let Some(p) = picked {
                        self.open_path(ctx, p);
                    }
                }
            });
        });
        ui.add_space(6.0);
        if delay_before != self.cfg.delay_secs {
            self.cfg.save();
        }
    }

    fn home(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(6.0);
        if !self.ffmpeg_ok {
            ui.colored_label(
                Color32::from_rgb(230, 180, 40),
                "ffmpeg was not found - recording and video editing need it. Install it or set its path in Settings.",
            );
        }
        ui.horizontal(|ui| {
            ui.heading("Recent");
            if ui.small_button("↻").on_hover_text("Refresh").clicked() {
                self.refresh_history();
            }
            if ui.small_button("📂").on_hover_text("Open folder").clicked() {
                let _ = std::fs::create_dir_all(&self.cfg.save_dir);
                let _ = open::that(&self.cfg.save_dir);
            }
        });
        ui.label("Press 📷 New to take a snip or start a recording. Captures open here so you can edit them.");
        ui.separator();
        let mut to_open: Option<PathBuf> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for p in &self.history {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                let tag = if is_video(p) { "🎬" } else { "🖼" };
                if ui
                    .selectable_label(false, format!("{tag}  {name}"))
                    .on_hover_text(name)
                    .clicked()
                {
                    to_open = Some(p.clone());
                }
            }
            if self.history.is_empty() {
                ui.label("Nothing here yet.");
            }
        });
        if let Some(p) = to_open {
            self.open_path(ctx, p);
        }
    }

    fn central(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        match self.view {
            View::Home => self.home(ui, ctx),
            View::Image => {
                if let Some(mut ed) = self.editor.take() {
                    match ed.ui(ui, &self.font) {
                        Some(Action::Close) => {
                            self.view = View::Home;
                        }
                        Some(a) => {
                            self.handle_editor_action(a, &mut ed);
                            self.editor = Some(ed);
                        }
                        None => {
                            self.editor = Some(ed);
                        }
                    }
                }
            }
            View::Video => {
                let mut close = false;
                ui.horizontal(|ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
                if close {
                    self.view = View::Home;
                    self.video = None;
                    return;
                }
                if let Some(mut v) = self.video.take() {
                    let act = v.ui(ui);
                    self.video = Some(v);
                    if let Some(a) = act {
                        self.handle_video_action(ctx, a);
                    }
                }
            }
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.strong("Snips");
            ui.label(format!("Save folder:\n{}", self.cfg.save_dir.display()));
            ui.horizontal(|ui| {
                if ui.button("Change...").clicked() {
                    if let Some(d) = rfd::FileDialog::new().set_directory(&self.cfg.save_dir).pick_folder() {
                        self.cfg.save_dir = d;
                        self.refresh_history();
                    }
                }
                if ui.button("Open").clicked() {
                    let _ = std::fs::create_dir_all(&self.cfg.save_dir);
                    let _ = open::that(&self.cfg.save_dir);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Format");
                ui.radio_value(&mut self.cfg.format, ImgFormat::Png, "PNG");
                ui.radio_value(&mut self.cfg.format, ImgFormat::Jpg, "JPG");
            });
            ui.add_enabled(
                self.cfg.format == ImgFormat::Jpg,
                egui::Slider::new(&mut self.cfg.jpg_quality, 40..=100).text("JPG quality"),
            );
            ui.checkbox(&mut self.cfg.auto_save, "Save every snip automatically");
            ui.checkbox(&mut self.cfg.auto_copy, "Copy every snip to the clipboard");
            ui.label("Tip: press Print Screen (Impr. ecran) anywhere to start a snip.");

            ui.add_space(8.0);
            ui.separator();
            ui.strong("Recording");
            egui::ComboBox::from_label("Frame rate")
                .selected_text(format!("{} fps", self.cfg.fps))
                .show_ui(ui, |ui| {
                    for f in [15u32, 24, 30, 60] {
                        ui.selectable_value(&mut self.cfg.fps, f, format!("{f} fps"));
                    }
                });
            ui.add(egui::Slider::new(&mut self.cfg.crf, 16..=34).text("CRF (lower = sharper)"));
            ui.checkbox(&mut self.cfg.record_audio, "Record audio");
            ui.add_enabled_ui(self.cfg.record_audio, |ui| {
                egui::ComboBox::from_label("Source")
                    .selected_text(audio_label(&self.cfg.audio_source))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.cfg.audio_source, MIC_DEFAULT.to_string(), "Default microphone");
                        ui.selectable_value(
                            &mut self.cfg.audio_source,
                            LOOPBACK.to_string(),
                            "System audio (loopback, Windows)",
                        );
                        for d in &self.audio_devices {
                            ui.selectable_value(&mut self.cfg.audio_source, d.clone(), d.as_str());
                        }
                    });
                if ui.small_button("Refresh devices").clicked() {
                    self.audio_devices = capture::list_audio_inputs();
                }
            });
            ui.label("ffmpeg path");
            let r = ui.add(egui::TextEdit::singleline(&mut self.cfg.ffmpeg_path).desired_width(f32::INFINITY));
            if r.lost_focus() {
                self.ffmpeg_ok = capture::ffmpeg_available(&self.cfg.ffmpeg_path);
            }
            ui.horizontal(|ui| {
                if ui.small_button("Check").clicked() {
                    self.ffmpeg_ok = capture::ffmpeg_available(&self.cfg.ffmpeg_path);
                }
                ui.label(if self.ffmpeg_ok { "ffmpeg found" } else { "ffmpeg NOT found" });
            });

            ui.add_space(8.0);
            ui.separator();
            ui.strong("Editor defaults");
            ui.horizontal(|ui| {
                ui.label("Color");
                ui.color_edit_button_srgb(&mut self.cfg.color);
            });
            ui.add(egui::Slider::new(&mut self.cfg.width, 1.0..=20.0).text("Line width"));
            ui.label("Text font file (.ttf / .otf, optional)");
            let r = ui.add(egui::TextEdit::singleline(&mut self.cfg.font_path).desired_width(f32::INFINITY));
            if r.lost_focus() {
                self.font = editor::load_font(&self.cfg.font_path);
            }

            ui.add_space(8.0);
            ui.separator();
            ui.strong("Background");
            ui.checkbox(
                &mut self.cfg.start_in_background,
                "Start in the background (tray icon only)",
            );
            ui.checkbox(
                &mut self.cfg.close_to_tray,
                "Closing the window hides it to the tray",
            );
            ui.checkbox(
                &mut self.cfg.autostart,
                "⚙ Start with Windows (runs in the tray)",
            );
            if self.tray.is_none() {
                ui.colored_label(
                    Color32::from_rgb(230, 180, 40),
                    "Tray icon unavailable - the window will stay open.",
                );
            } else if ui.button("Hide to tray now").clicked() {
                self.hide_to_tray(ui.ctx());
            }
            ui.label("Press Print Screen from anywhere to start a snip.");

            ui.add_space(8.0);
            ui.separator();
            ui.weak(format!("SnipXTool {}", env!("CARGO_PKG_VERSION")));
        });
        // persist whenever the user finishes an interaction in this panel
        if ui.input(|i| i.pointer.any_released()) {
            self.cfg.save();
            self.sync_autostart();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        theme::apply(ctx);
        // eframe shows the window after its first frame regardless of the
        // initial `visible` flag, so keep it hidden while we are in tray mode.
        if self.hidden && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if self.cfg.close_to_tray && self.tray.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_to_tray(ctx);
                return;
            }
        }
        if self.hwnd.is_none() {
            self.hwnd = platform::hwnd(frame);
            if let (Some(t), Some(h)) = (self.tray.as_ref(), self.hwnd) {
                t.set_hwnd(h);
            }
        }
        if !self.hotkey_started {
            self.hotkey_started = true;
            let (tx, c) = (self.tx.clone(), ctx.clone());
            platform::start_hotkey(self.hwnd, move |ev| {
                let m = match ev {
                    platform::HotkeyEvent::Pressed { was_minimized } => Msg::Hotkey { was_minimized },
                    platform::HotkeyEvent::Activated => Msg::Activated,
                    platform::HotkeyEvent::Failed(e) => Msg::HotkeyFailed(e),
                };
                let _ = tx.send(m);
                c.request_repaint();
            });
        }
        // Apply the "start with Windows" preference once per session (and
        // whenever it is toggled in Settings) so the installed exe is used.
        if !self.autostart_synced {
            self.autostart_synced = true;
            #[cfg(windows)]
            platform::autostart_remove_legacy();
            self.sync_autostart();
        }
        self.poll(ctx);
        self.apply_geom(ctx);
        self.drive(ctx);

        let tray_actions = self.tray.as_ref().map(|t| t.actions()).unwrap_or_default();
        for action in tray_actions {
            match action {
                tray::TrayAction::Open => self.show_window(ctx),
                tray::TrayAction::Snip => {
                    self.show_window(ctx);
                    self.tab = Tab::Snip;
                    if matches!(self.phase, Phase::Idle) {
                        self.new_clicked(ctx, Duration::ZERO);
                    }
                }
                tray::TrayAction::Settings => {
                    self.show_window(ctx);
                    self.show_settings = true;
                }
                tray::TrayAction::Quit => {
                    self.quit(ctx);
                    return;
                }
            }
        }

        // Dropping an image/video onto the window opens it.
        if matches!(self.phase, Phase::Idle) {
            let dropped = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .find_map(|f| f.path.clone())
            });
            if let Some(path) = dropped {
                self.open_path(ctx, path);
            }
        }

        let kind = match self.phase {
            Phase::Selecting(_) => 1,
            Phase::Countdown { .. } | Phase::Recording => 2,
            _ => 0,
        };
        if kind == 1 {
            self.selection_ui(ctx);
            return;
        }
        if kind == 2 {
            self.pill_ui(ctx);
            return;
        }

        egui::TopBottomPanel::top("bar").show(ctx, |ui| self.top_bar(ui, ctx));
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.label(&self.status);
        });
        if self.show_settings {
            egui::SidePanel::right("settings")
                .resizable(true)
                .default_width(300.0)
                .show(ctx, |ui| self.settings_ui(ui));
        }
        egui::CentralPanel::default().show(ctx, |ui| self.central(ui, ctx));
    }
}

// ───────────── free functions ─────────────

fn region_from(r: Rect, sw: u32, sh: u32) -> Option<Region> {
    let x = (r.min.x.max(0.0) as u32).min(sw.saturating_sub(1));
    let y = (r.min.y.max(0.0) as u32).min(sh.saturating_sub(1));
    let w = (r.width() as u32).min(sw - x) & !1;
    let h = (r.height() as u32).min(sh - y) & !1;
    if w >= 16 && h >= 16 {
        Some(Region { x, y, w, h })
    } else {
        None
    }
}

fn audio_label(s: &str) -> String {
    match s {
        MIC_DEFAULT => "Default microphone".to_string(),
        LOOPBACK => "System audio (loopback, Windows)".to_string(),
        other => other.to_string(),
    }
}

fn is_video(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("mp4")).unwrap_or(false)
}

/// Most recent captures in the save folder (images + mp4), newest first.
fn scan_dir(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    let name = p.file_name()?.to_str()?.to_string();
                    if name.contains(".tmp.") {
                        return None;
                    }
                    let ext = p.extension()?.to_str()?.to_lowercase();
                    if ["png", "jpg", "jpeg", "mp4"].contains(&ext.as_str()) {
                        Some((e.metadata().ok()?.modified().ok()?, p))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().take(60).map(|x| x.1).collect()
}

/// Decodes the bundled logo and scales it to a square `size`x`size` RGBA buffer.
fn logo_rgba(size: u32) -> (Vec<u8>, u32, u32) {
    // Load the image directly into the executable at compile time
    let bytes = include_bytes!("../assets/logo.png");
    let img = image::load_from_memory(bytes)
        .expect("Failed to load logo.png")
        .to_rgba8();
    let img = if img.width() == size && img.height() == size {
        img
    } else {
        image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3)
    };
    (img.into_raw(), size, size)
}

fn load_icon() -> egui::IconData {
    let (rgba, width, height) = logo_rgba(256);
    egui::IconData {
        rgba,
        width,
        height,
    }
}