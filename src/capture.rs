//! Screenshots, screen recording (frames -> ffmpeg), audio capture (cpal -> WAV)
//! and the ffmpeg helpers used by the video editor.

use crate::settings::{Settings, LOOPBACK, MIC_DEFAULT};
use image::RgbaImage;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

// ───────────────────────────── ffmpeg helpers ─────────────────────────────

#[allow(unused_mut)]
pub fn ffmpeg_cmd(bin: &str) -> Command {
    let mut c = Command::new(if bin.trim().is_empty() { "ffmpeg" } else { bin });
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

pub fn ffmpeg_available(bin: &str) -> bool {
    ffmpeg_cmd(bin)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Reads "Duration: HH:MM:SS.xx" from `ffmpeg -i` (no ffprobe needed).
pub fn probe_duration(ff: &str, input: &Path) -> Option<f64> {
    let out = ffmpeg_cmd(ff).arg("-hide_banner").arg("-i").arg(input).output().ok()?;
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    let i = text.find("Duration: ")? + 10;
    let t = text.get(i..i + 11)?;
    let mut it = t.split(':');
    let h: f64 = it.next()?.parse().ok()?;
    let m: f64 = it.next()?.parse().ok()?;
    let s: f64 = it.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

/// Decodes a single frame at `t` seconds as an image.
pub fn grab_frame(ff: &str, input: &Path, t: f64) -> Result<RgbaImage, String> {
    let out = ffmpeg_cmd(ff)
        .args(["-v", "error"])
        .arg("-ss")
        .arg(format!("{t:.3}"))
        .arg("-i")
        .arg(input)
        .args(["-frames:v", "1", "-f", "image2pipe", "-vcodec", "png", "-"])
        .output()
        .map_err(|e| e.to_string())?;
    if out.stdout.is_empty() {
        return Err("ffmpeg returned no frame".into());
    }
    image::load_from_memory(&out.stdout)
        .map(|i| i.to_rgba8())
        .map_err(|e| e.to_string())
}

/// Trim (and optionally mute / convert to GIF) a clip.
#[allow(clippy::too_many_arguments)]
pub fn export_clip(
    ff: &str,
    input: &Path,
    output: &Path,
    start: f64,
    end: f64,
    mute: bool,
    crf: u32,
    gif: bool,
) -> Result<PathBuf, String> {
    let mut c = ffmpeg_cmd(ff);
    c.args(["-y", "-v", "error"]);
    c.arg("-ss").arg(format!("{start:.3}"));
    c.arg("-i").arg(input);
    c.arg("-t").arg(format!("{:.3}", (end - start).max(0.1)));
    if gif {
        c.args([
            "-vf",
            "fps=12,scale=720:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse",
            "-loop",
            "0",
        ]);
    } else {
        c.args(["-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p"]);
        c.arg("-crf").arg(crf.to_string());
        if mute {
            c.arg("-an");
        } else {
            c.args(["-c:a", "aac", "-b:a", "192k"]);
        }
    }
    c.arg(output);
    let out = c.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(output.to_path_buf())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

// ───────────────────────────── screenshots ─────────────────────────────

/// Position/size of a monitor in physical pixels (virtual-desktop coordinates).
#[derive(Clone, Copy, Default)]
pub struct MonInfo {
    pub idx: usize,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// The monitor containing the pixel position `px` (falls back to the primary one).
pub fn monitor_at(px: Option<(f32, f32)>) -> MonInfo {
    let mons = xcap::Monitor::all().unwrap_or_default();
    let info = |i: usize, m: &xcap::Monitor| MonInfo {
        idx: i,
        x: m.x() as i32,
        y: m.y() as i32,
        w: m.width() as u32,
        h: m.height() as u32,
    };
    if let Some((px, py)) = px {
        for (i, m) in mons.iter().enumerate() {
            let mi = info(i, m);
            if px >= mi.x as f32
                && px < (mi.x + mi.w as i32) as f32
                && py >= mi.y as f32
                && py < (mi.y + mi.h as i32) as f32
            {
                return mi;
            }
        }
    }
    for (i, m) in mons.iter().enumerate() {
        if m.is_primary() {
            return info(i, m);
        }
    }
    mons.first().map(|m| info(0, m)).unwrap_or_default()
}

/// Visible top-level windows on that monitor as [x0, y0, x1, y1] in screenshot pixels,
/// top-most first (used by "Window" mode).
pub fn list_windows(mon: MonInfo) -> Vec<[f32; 4]> {
    let wins = xcap::Window::all().unwrap_or_default();
    let mut out = Vec::new();
    for w in wins.iter() {
        if w.is_minimized() {
            continue;
        }
        let title = w.title().to_string();
        if title.trim().is_empty() || title == "SnipXTool" {
            continue;
        }
        let x = w.x() as i32 - mon.x;
        let y = w.y() as i32 - mon.y;
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w.width() as i32).min(mon.w as i32);
        let y1 = (y + w.height() as i32).min(mon.h as i32);
        if x1 - x0 < 40 || y1 - y0 < 40 {
            continue;
        }
        out.push([x0 as f32, y0 as f32, x1 as f32, y1 as f32]);
    }
    out
}

/// Crops `[x0, y0, x1, y1]` (pixels) out of the screenshot.
pub fn crop(img: &RgbaImage, r: [f32; 4]) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    let x0 = (r[0].max(0.0) as u32).min(w.saturating_sub(1));
    let y0 = (r[1].max(0.0) as u32).min(h.saturating_sub(1));
    let x1 = (r[2].max(0.0) as u32).min(w).max(x0 + 1);
    let y1 = (r[3].max(0.0) as u32).min(h).max(y0 + 1);
    image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image()
}

/// Freeform snip: keeps only the pixels inside the drawn shape; the rest is transparent.
pub fn freeform_cut(img: &RgbaImage, pts: &[(f32, f32)]) -> Option<RgbaImage> {
    if pts.len() < 3 {
        return None;
    }
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let minx = pts.iter().map(|p| p.0).fold(f32::MAX, f32::min).clamp(0.0, iw).floor() as u32;
    let maxx = pts.iter().map(|p| p.0).fold(f32::MIN, f32::max).clamp(0.0, iw).ceil() as u32;
    let miny = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min).clamp(0.0, ih).floor() as u32;
    let maxy = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max).clamp(0.0, ih).ceil() as u32;
    if maxx <= minx + 1 || maxy <= miny + 1 {
        return None;
    }
    let (w, h) = (maxx - minx, maxy - miny);
    let mut out = RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 0]));
    for y in 0..h {
        let fy = (miny + y) as f32 + 0.5;
        let mut xs: Vec<f32> = Vec::new();
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            if (a.1 <= fy && b.1 > fy) || (b.1 <= fy && a.1 > fy) {
                xs.push(a.0 + (fy - a.1) / (b.1 - a.1) * (b.0 - a.0));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in xs.chunks(2) {
            if pair.len() == 2 {
                let x0 = (pair[0] - minx as f32).max(0.0).round() as u32;
                let x1 = ((pair[1] - minx as f32).round().max(0.0) as u32).min(w);
                for x in x0..x1 {
                    out.put_pixel(x, y, *img.get_pixel(minx + x, miny + y));
                }
            }
        }
    }
    Some(out)
}

fn crop_raw(src: &[u8], sw: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in y..y + h {
        let s = ((row * sw + x) * 4) as usize;
        out.extend_from_slice(&src[s..s + (w * 4) as usize]);
    }
    out
}

pub fn grab_screen(idx: usize) -> Result<RgbaImage, String> {
    let mons = xcap::Monitor::all().map_err(|e| e.to_string())?;
    let mon = mons.get(idx).or_else(|| mons.first()).ok_or("no monitor found")?;
    let img = mon.capture_image().map_err(|e| e.to_string())?;
    let (w, h) = (img.width(), img.height());
    // rebuild from raw bytes so we don't depend on xcap's `image` version
    RgbaImage::from_raw(w, h, img.into_raw()).ok_or_else(|| "invalid frame".to_string())
}

// ───────────────────────────── audio ─────────────────────────────

pub fn list_audio_inputs() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    cpal::default_host()
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

/// Writes samples to a WAV and pads gaps with silence, because WASAPI loopback
/// delivers no data while nothing is playing (that would break A/V sync).
struct Sink {
    wr: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
    written: u64, // frames (one sample per channel)
    start: Instant,
    rate: u32,
    ch: u16,
}

impl Sink {
    fn push(&mut self, samples: &[i16]) {
        let Some(wr) = self.wr.as_mut() else { return };
        let ch = self.ch.max(1) as u64;
        let incoming = samples.len() as u64 / ch;
        let expected = (self.start.elapsed().as_secs_f64() * self.rate as f64) as u64;
        if expected > self.written + incoming + (self.rate as u64) / 10 {
            let gap = expected - self.written - incoming;
            for _ in 0..gap * ch {
                let _ = wr.write_sample(0i16);
            }
            self.written += gap;
        }
        for s in samples {
            let _ = wr.write_sample(*s);
        }
        self.written += incoming;
    }
}

fn record_audio(src: &str, path: &Path, stop: Arc<AtomicBool>, ready: Arc<AtomicBool>) -> Result<(), String> {
    let r = record_audio_inner(src, path, &stop, &ready);
    ready.store(true, Ordering::Relaxed); // never leave the video thread waiting
    r
}

fn record_audio_inner(src: &str, path: &Path, stop: &AtomicBool, ready: &AtomicBool) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();

    let (device, supported) = if src == LOOPBACK {
        // On Windows, building an *input* stream on an *output* device = loopback.
        let d = host.default_output_device().ok_or("no output device for loopback")?;
        let c = d.default_output_config().map_err(|e| e.to_string())?;
        (d, c)
    } else {
        let d = (if src == MIC_DEFAULT || src.is_empty() {
            host.default_input_device()
        } else {
            host.input_devices()
                .ok()
                .and_then(|mut it| it.find(|d| d.name().map(|n| n == src).unwrap_or(false)))
        })
        .ok_or("audio input device not found")?;
        let c = d.default_input_config().map_err(|e| e.to_string())?;
        (d, c)
    };

    let rate = supported.sample_rate().0;
    let ch = supported.channels();
    let fmt = supported.sample_format();
    let stream_cfg: cpal::StreamConfig = supported.into();

    let spec = hound::WavSpec {
        channels: ch,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let wr = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    let sink = Arc::new(Mutex::new(Sink { wr: Some(wr), written: 0, start: Instant::now(), rate, ch }));
    let s2 = sink.clone();
    let err_fn = |e: cpal::StreamError| eprintln!("audio stream error: {e}");

    let stream = (match fmt {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &stream_cfg,
            move |d: &[f32], _: &cpal::InputCallbackInfo| {
                let v: Vec<i16> = d
                    .iter()
                    .map(|s| {
                        let x = *s;
                        (x.clamp(-1.0, 1.0) * 32767.0) as i16
                    })
                    .collect();
                if let Ok(mut g) = s2.lock() {
                    g.push(&v);
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &stream_cfg,
            move |d: &[i16], _: &cpal::InputCallbackInfo| {
                if let Ok(mut g) = s2.lock() {
                    g.push(d);
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &stream_cfg,
            move |d: &[u16], _: &cpal::InputCallbackInfo| {
                let v: Vec<i16> = d.iter().map(|s| (*s as i32 - 32768) as i16).collect();
                if let Ok(mut g) = s2.lock() {
                    g.push(&v);
                }
            },
            err_fn,
            None,
        ),
        other => return Err(format!("unsupported audio sample format {other:?}")),
    })
    .map_err(|e| e.to_string())?;

    stream.play().map_err(|e| e.to_string())?;
    ready.store(true, Ordering::Relaxed);
    while !stop.load(Ordering::Relaxed) {
        sleep(Duration::from_millis(30));
    }
    drop(stream);
    if let Ok(mut g) = sink.lock() {
        if let Some(w) = g.wr.take() {
            w.finalize().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ───────────────────────────── recording ─────────────────────────────

pub struct Recorder {
    pub stop: Arc<AtomicBool>,
    pub started: Instant,
}

/// `Ok((file, optional warning))`
pub type RecordResult = Result<(PathBuf, Option<String>), String>;

/// `region` = (x, y, width, height) in monitor pixels, or None for the whole screen.
pub fn start_recording(
    cfg: Settings,
    out: PathBuf,
    mon_idx: usize,
    region: Option<(u32, u32, u32, u32)>,
    lead: Duration,
    on_done: impl FnOnce(RecordResult) + Send + 'static,
) -> Recorder {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        sleep(lead);
        on_done(record(cfg, out, flag, mon_idx, region));
    });
    Recorder { stop, started: Instant::now() }
}

fn record(
    cfg: Settings,
    out: PathBuf,
    stop: Arc<AtomicBool>,
    mon_idx: usize,
    region: Option<(u32, u32, u32, u32)>,
) -> RecordResult {
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mons = xcap::Monitor::all().map_err(|e| e.to_string())?;
    let mon = mons.get(mon_idx).or_else(|| mons.first()).ok_or("no monitor found")?;
    let first = mon.capture_image().map_err(|e| e.to_string())?;
    let (sw, sh) = (first.width(), first.height());
    // recorded area (even size, required by H.264 yuv420p)
    let (rx, ry, w, h) = match region {
        Some((x, y, w, h)) if w >= 2 && h >= 2 && x + w <= sw && y + h <= sh => (x, y, w & !1, h & !1),
        _ => (0, 0, sw & !1, sh & !1),
    };
    let whole = (rx, ry, w, h) == (0, 0, sw, sh);
    let grab = |raw: Vec<u8>| -> Vec<u8> {
        if whole {
            raw
        } else {
            crop_raw(&raw, sw, rx, ry, w, h)
        }
    };
    let mut frame = grab(first.into_raw());

    let fps_i = cfg.fps.clamp(5, 120);
    let fps = fps_i as f64;
    let tmp_video = out.with_extension("video.tmp.mp4");
    let tmp_audio = out.with_extension("audio.tmp.wav");

    // raw RGBA frames -> ffmpeg stdin -> H.264
    let mut child = ffmpeg_cmd(&cfg.ffmpeg_path)
        .args(["-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgba"])
        .arg("-s")
        .arg(format!("{w}x{h}"))
        .arg("-framerate")
        .arg(fps_i.to_string())
        .args(["-i", "-", "-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2"])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"])
        .arg("-crf")
        .arg(cfg.crf.to_string())
        .arg(&tmp_video)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start ffmpeg ({e}) - install it or set its path in Settings"))?;
    let mut stdin = child.stdin.take().ok_or("ffmpeg stdin unavailable")?;

    // audio on its own thread (cpal streams are not Send)
    let ready = Arc::new(AtomicBool::new(false));
    let audio = if cfg.record_audio {
        let (src, p, st, rd) = (cfg.audio_source.clone(), tmp_audio.clone(), stop.clone(), ready.clone());
        Some(std::thread::spawn(move || record_audio(&src, &p, st, rd)))
    } else {
        None
    };
    if audio.is_some() {
        let t0 = Instant::now();
        while !ready.load(Ordering::Relaxed) && t0.elapsed() < Duration::from_secs(3) {
            sleep(Duration::from_millis(10));
        }
    }

    // frame loop: duplicate frames when capture is slower than the target fps,
    // so the video length always matches wall-clock time (and the audio).
    let start = Instant::now();
    let mut written: u64 = 0;
    let mut write_err: Option<String> = None;
    while !stop.load(Ordering::Relaxed) {
        if let Ok(img) = mon.capture_image() {
            if img.width() == sw && img.height() == sh {
                frame = grab(img.into_raw());
            }
        }
        let due = (start.elapsed().as_secs_f64() * fps) as u64 + 1;
        while written < due {
            if let Err(e) = stdin.write_all(&frame) {
                write_err = Some(e.to_string());
                break;
            }
            written += 1;
        }
        if write_err.is_some() {
            break;
        }
        let next = start + Duration::from_secs_f64(written as f64 / fps);
        if let Some(d) = next.checked_duration_since(Instant::now()) {
            sleep(d);
        }
    }

    stop.store(true, Ordering::Relaxed); // also stops audio if ffmpeg died
    drop(stdin);
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let audio_res: Option<Result<(), String>> =
        audio.map(|h| h.join().unwrap_or_else(|_| Err("audio thread panicked".to_string())));

    if let Some(e) = write_err {
        return Err(format!("ffmpeg stopped: {e} {}", String::from_utf8_lossy(&output.stderr)));
    }
    if !output.status.success() {
        return Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&output.stderr)));
    }

    let mut warning = None;
    if let Some(Err(e)) = &audio_res {
        warning = Some(format!("audio not recorded: {e}"));
    }
    let have_audio = matches!(audio_res, Some(Ok(())))
        && tmp_audio.metadata().map(|m| m.len() > 100).unwrap_or(false);

    if have_audio {
        let mux = ffmpeg_cmd(&cfg.ffmpeg_path)
            .args(["-y", "-v", "error", "-i"])
            .arg(&tmp_video)
            .arg("-i")
            .arg(&tmp_audio)
            .args(["-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-shortest"])
            .arg(&out)
            .output()
            .map_err(|e| e.to_string())?;
        if !mux.status.success() {
            return Err(format!("muxing failed: {}", String::from_utf8_lossy(&mux.stderr)));
        }
        let _ = std::fs::remove_file(&tmp_video);
    } else {
        std::fs::rename(&tmp_video, &out).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(&tmp_audio);
    Ok((out, warning))
}
