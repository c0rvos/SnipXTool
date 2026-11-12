//! Screenshot editor: pen, line, arrow, rectangle, ellipse, highlight, blur,
//! text and crop, with undo/redo. What you see on the canvas is exactly what
//! gets saved, because the preview *is* the rasterised result.

use crate::settings::Settings;
use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use image::{Rgba, RgbaImage};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Pen,
    Line,
    Arrow,
    Rect,
    Ellipse,
    Highlight,
    Pixelate,
    Text,
    Step,
    Crop,
}

/// (tool, label with a symbolic glyph). Glyphs are chosen from the fonts egui
/// bundles (Noto Emoji / emoji-icon) plus Segoe UI, so they always render.
const TOOLS: [(Tool, &str); 10] = [
    (Tool::Pen, "✏ Pen"),
    (Tool::Line, "— Line"),
    (Tool::Arrow, "→ Arrow"),
    (Tool::Rect, "□ Rect"),
    (Tool::Ellipse, "◯ Ellipse"),
    (Tool::Highlight, "▬ Highlight"),
    (Tool::Pixelate, "▓ Blur"),
    (Tool::Text, "T Text"),
    (Tool::Step, "🎯 Steps"),
    (Tool::Crop, "⛶ Crop"),
];

/// One-click color swatches shown next to the color picker.
const SWATCHES: [[u8; 3]; 7] = [
    [230, 40, 40],
    [255, 160, 0],
    [70, 170, 60],
    [0, 120, 215],
    [160, 60, 220],
    [255, 255, 255],
    [20, 20, 20],
];

fn tool_tip(t: Tool) -> &'static str {
    match t {
        Tool::Pen => "Freehand pen",
        Tool::Line => "Straight line",
        Tool::Arrow => "Arrow",
        Tool::Rect => "Rectangle outline",
        Tool::Ellipse => "Ellipse outline",
        Tool::Highlight => "Translucent highlighter",
        Tool::Pixelate => "Blur / pixelate region",
        Tool::Text => "Add text",
        Tool::Step => "Numbered step badges (click where each step happens)",
        Tool::Crop => "Crop the image",
    }
}

pub enum Action {
    Save,
    SaveAs,
    Copy,
    Close,
}

#[derive(Clone)]
struct Shape {
    kind: Tool,
    pts: Vec<Pos2>, // image coordinates
    color: [u8; 3],
    width: f32, // stroke width, or font size for text
    text: String,
}

struct EditState {
    base: Arc<RgbaImage>,
    shapes: Vec<Shape>,
}

pub struct Editor {
    base: Arc<RgbaImage>,
    shapes: Vec<Shape>,
    undo: Vec<EditState>,
    redo: Vec<EditState>,
    tex: Option<egui::TextureHandle>,
    dirty: bool,
    pub tool: Tool,
    color: [u8; 3],
    width: f32,
    text_size: f32,
    step_no: u32,
    current: Option<Shape>,
    anchor: Pos2,
    pending_text: Option<(Pos2, String)>,
    crop: Option<Rect>,
    zoom: Option<f32>,
    last_fit: f32,
    pub path: Option<PathBuf>,
}

impl Editor {
    pub fn new(img: RgbaImage, cfg: &Settings) -> Self {
        Self {
            base: Arc::new(img),
            shapes: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            tex: None,
            dirty: true,
            tool: Tool::Pen,
            color: cfg.color,
            width: cfg.width,
            text_size: 28.0,
            step_no: 1,
            current: None,
            anchor: Pos2::ZERO,
            pending_text: None,
            crop: None,
            zoom: None,
            last_fit: 1.0,
            path: None,
        }
    }

    pub fn flattened(&self, font: &Option<FontArc>) -> RgbaImage {
        render(&self.base, &self.shapes, font)
    }

    // ── history ──
    fn snapshot(&self) -> EditState {
        EditState { base: self.base.clone(), shapes: self.shapes.clone() }
    }
    fn push_undo(&mut self) {
        let s = self.snapshot();
        self.undo.push(s);
        self.redo.clear();
    }
    fn restore(&mut self, s: EditState) {
        if self.base.dimensions() != s.base.dimensions() {
            self.zoom = None;
        }
        self.base = s.base;
        self.shapes = s.shapes;
        self.current = None;
        self.crop = None;
        self.dirty = true;
    }
    fn do_undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            let cur = self.snapshot();
            self.redo.push(cur);
            self.restore(s);
        }
    }
    fn do_redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            let cur = self.snapshot();
            self.undo.push(cur);
            self.restore(s);
        }
    }

    fn refresh(&mut self, ctx: &egui::Context, font: &Option<FontArc>) {
        if !self.dirty && self.tex.is_some() {
            return;
        }
        let img = render(&self.base, &self.shapes, font);
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.as_raw(),
        );
        let opts = egui::TextureOptions::LINEAR;
        if let Some(t) = self.tex.as_mut() {
            t.set(ci, opts);
        } else {
            self.tex = Some(ctx.load_texture("edit", ci, opts));
        }
        self.dirty = false;
    }

    fn apply_crop(&mut self, font: &Option<FontArc>) {
        let Some(r) = self.crop.take() else { return };
        let (w, h) = (self.base.width(), self.base.height());
        let x = (r.min.x.max(0.0) as u32).min(w - 1);
        let y = (r.min.y.max(0.0) as u32).min(h - 1);
        let cw = (r.width() as u32).min(w - x);
        let ch = (r.height() as u32).min(h - y);
        if cw < 4 || ch < 4 {
            return;
        }
        let flat = render(&self.base, &self.shapes, font); // bake annotations first
        self.push_undo();
        self.base = Arc::new(image::imageops::crop_imm(&flat, x, y, cw, ch).to_image());
        self.shapes.clear();
        self.zoom = None;
        self.dirty = true;
    }

    fn new_shape(&self, pts: Vec<Pos2>) -> Shape {
        Shape { kind: self.tool, pts, color: self.color, width: self.width, text: String::new() }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, font: &Option<FontArc>) -> Option<Action> {
        let ctx = ui.ctx().clone();
        self.refresh(&ctx, font);
        let mut action: Option<Action> = None;
        let mut want_crop = false;

        // ── keyboard shortcuts ──
        if !ctx.wants_keyboard_input() {
            let (undo, redo, save, copy, enter, esc) = ctx.input(|i| {
                let cmd = i.modifiers.command;
                let z = i.key_pressed(egui::Key::Z);
                (
                    cmd && z && !i.modifiers.shift,
                    cmd && ((z && i.modifiers.shift) || i.key_pressed(egui::Key::Y)),
                    cmd && i.key_pressed(egui::Key::S),
                    i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if undo {
                self.do_undo();
            }
            if redo {
                self.do_redo();
            }
            if save {
                action = Some(Action::Save);
            }
            if copy {
                action = Some(Action::Copy);
            }
            if enter && self.crop.is_some() {
                want_crop = true;
            }
            if esc {
                self.crop = None;
                self.current = None;
            }
        }

        // ── toolbar ──
        ui.horizontal_wrapped(|ui| {
            for (t, name) in TOOLS {
                if ui
                    .selectable_label(self.tool == t, name)
                    .on_hover_text(tool_tip(t))
                    .clicked()
                {
                    self.tool = t;
                    if t != Tool::Crop {
                        self.crop = None;
                    }
                }
            }
            ui.separator();
            ui.label("🎨").on_hover_text("Annotation color");
            ui.color_edit_button_srgb(&mut self.color);
            for c in SWATCHES {
                let resp = ui.allocate_response(Vec2::splat(16.0), egui::Sense::click());
                ui.painter()
                    .rect_filled(resp.rect, 2.0, Color32::from_rgb(c[0], c[1], c[2]));
                if resp.hovered() {
                    ui.painter()
                        .rect_stroke(resp.rect, 2.0, Stroke::new(1.5_f32, Color32::WHITE));
                }
                if resp.clicked() {
                    self.color = c;
                }
            }
            if self.tool == Tool::Text {
                ui.add(egui::Slider::new(&mut self.text_size, 12.0..=120.0).text("text size"));
            } else if self.tool == Tool::Step {
                ui.add(egui::Slider::new(&mut self.width, 2.0..=40.0).text("badge size"));
                if ui.button("↻ 1").on_hover_text("Restart numbering at 1").clicked() {
                    self.step_no = 1;
                }
            } else {
                ui.add(egui::Slider::new(&mut self.width, 1.0..=40.0).text("width"));
            }
            ui.separator();
            if ui
                .add_enabled(!self.undo.is_empty(), egui::Button::new("↩ Undo"))
                .clicked()
            {
                self.do_undo();
            }
            if ui
                .add_enabled(!self.redo.is_empty(), egui::Button::new("↪ Redo"))
                .clicked()
            {
                self.do_redo();
            }
            if self.crop.is_some() {
                if ui.button("⛶ Apply crop (Enter)").clicked() {
                    want_crop = true;
                }
                if ui.button("× Cancel").clicked() {
                    self.crop = None;
                }
            }
            ui.separator();
            if ui.button("Fit").on_hover_text("Fit to window").clicked() {
                self.zoom = None;
            }
            if ui.button("1:1").on_hover_text("Actual size").clicked() {
                self.zoom = Some(1.0);
            }
            if ui.button("-").on_hover_text("Zoom out").clicked() {
                self.zoom = Some((self.zoom.unwrap_or(self.last_fit) / 1.25).max(0.05));
            }
            if ui.button("+").on_hover_text("Zoom in").clicked() {
                self.zoom = Some((self.zoom.unwrap_or(self.last_fit) * 1.25).min(8.0));
            }
            ui.separator();
            if ui.button("⎘ Copy").clicked() {
                action = Some(Action::Copy);
            }
            if ui.button("💾 Save").clicked() {
                action = Some(Action::Save);
            }
            if ui.button("📝 Save as...").clicked() {
                action = Some(Action::SaveAs);
            }
            if ui.button("× Close").clicked() {
                action = Some(Action::Close);
            }
        });
        if want_crop {
            self.apply_crop(font);
            self.refresh(&ctx, font);
        }
        ui.separator();

        // ── canvas ──
        let (iw, ih) = (self.base.width() as f32, self.base.height() as f32);
        let avail = ui.available_size();
        let fit = ((avail.x - 6.0) / iw).min((avail.y - 6.0) / ih).clamp(0.02, 1.0);
        self.last_fit = fit;
        let scale = self.zoom.unwrap_or(fit);
        let tex_id = self.tex.as_ref().map(|t| t.id());

        egui::ScrollArea::both().auto_shrink([false, false]).drag_to_scroll(false).show(ui, |ui| {
            let (resp, painter) =
                ui.allocate_painter(Vec2::new(iw * scale, ih * scale), Sense::click_and_drag());
            let rect = resp.rect;
            if let Some(id) = tex_id {
                painter.image(id, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            let to_img = |p: Pos2| {
                let v = (p - rect.min) / scale;
                Pos2::new(v.x.clamp(0.0, iw), v.y.clamp(0.0, ih))
            };
            let to_scr = |p: Pos2| rect.min + p.to_vec2() * scale;

            if resp.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            }

            if let Some(p) = resp.interact_pointer_pos() {
                let ip = to_img(p);
                match self.tool {
                    Tool::Text => {
                        if resp.clicked() {
                            self.pending_text = Some((ip, String::new()));
                        }
                    }
                    Tool::Step => {
                        if resp.clicked() {
                            let mut s = self.new_shape(vec![ip]);
                            s.kind = Tool::Step;
                            s.width = self.width;
                            s.text = self.step_no.to_string();
                            self.step_no += 1;
                            self.push_undo();
                            self.shapes.push(s);
                            self.dirty = true;
                        }
                    }
                    Tool::Crop => {
                        if resp.drag_started() {
                            self.anchor = ip;
                            self.crop = Some(Rect::from_two_pos(ip, ip));
                        } else if resp.dragged() {
                            self.crop = Some(Rect::from_two_pos(self.anchor, ip));
                        }
                    }
                    Tool::Pen => {
                        if resp.drag_started() {
                            self.current = Some(self.new_shape(vec![ip]));
                        } else if resp.dragged() {
                            if let Some(s) = self.current.as_mut() {
                                if s.pts.last().map_or(true, |l| l.distance(ip) > 0.5) {
                                    s.pts.push(ip);
                                }
                            }
                        }
                    }
                    _ => {
                        if resp.drag_started() {
                            self.current = Some(self.new_shape(vec![ip, ip]));
                        } else if resp.dragged() {
                            if let Some(s) = self.current.as_mut() {
                                s.pts[1] = ip;
                            }
                        }
                    }
                }
            }

            if resp.drag_stopped() {
                if let Some(s) = self.current.take() {
                    let ok = match s.kind {
                        Tool::Pen => !s.pts.is_empty(),
                        _ => s.pts.len() == 2 && s.pts[0].distance(s.pts[1]) > 2.0,
                    };
                    if ok {
                        self.push_undo();
                        self.shapes.push(s);
                        self.dirty = true;
                    }
                }
            }

            if let Some(s) = &self.current {
                draw_preview(&painter, s, &to_scr, scale);
            }

            if let Some(c) = self.crop {
                let r = Rect::from_min_max(to_scr(c.min), to_scr(c.max));
                let dim = Color32::from_black_alpha(140);
                painter.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.max.x, r.min.y)), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, r.max.y), rect.max), 0.0, dim);
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(rect.min.x, r.min.y), Pos2::new(r.min.x, r.max.y)),
                    0.0,
                    dim,
                );
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(r.max.x, r.min.y), Pos2::new(rect.max.x, r.max.y)),
                    0.0,
                    dim,
                );
                painter.rect_stroke(r, 0.0, Stroke::new(1.5_f32, Color32::WHITE));
            }
        });

        // ── text entry dialog ──
        if let Some((pos, mut text)) = self.pending_text.take() {
            let mut done = false;
            let mut add = false;
            let (commit, cancel) = ctx.input(|i| {
                (
                    i.modifiers.command && i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if commit {
                add = true;
                done = true;
            }
            if cancel {
                done = true;
            }
            egui::Window::new("Add text")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(&ctx, |ui| {
                    let r = ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .desired_rows(3)
                            .desired_width(300.0),
                    );
                    if !r.has_focus() {
                        r.request_focus();
                    }
                    ui.horizontal(|ui| {
                        if ui.button("✔ Add").clicked() {
                            add = true;
                            done = true;
                        }
                        if ui.button("× Cancel").clicked() {
                            done = true;
                        }
                        ui.weak("Ctrl+Enter adds · Esc cancels");
                    });
                });
            let final_text = text
                .trim_end_matches(|c: char| c == '\r' || c == '\n')
                .to_string();
            if add && !final_text.trim().is_empty() {
                let mut s = self.new_shape(vec![pos]);
                s.kind = Tool::Text;
                s.width = self.text_size;
                s.text = final_text;
                self.push_undo();
                self.shapes.push(s);
                self.dirty = true;
            } else if !done {
                self.pending_text = Some((pos, text));
            }
        }

        action
    }
}

// ───────────────────────────── geometry helpers ─────────────────────────────

fn arrow_segments(a: Pos2, b: Pos2, w: f32) -> [(Pos2, Pos2); 3] {
    let d = b - a;
    let len = d.length().max(0.001);
    let u = d / len;
    let head = (w * 4.0).max(14.0).min(len * 0.6);
    let rot = |v: Vec2, ang: f32| {
        Vec2::new(v.x * ang.cos() - v.y * ang.sin(), v.x * ang.sin() + v.y * ang.cos())
    };
    let l = rot(-u, 0.5) * head;
    let r = rot(-u, -0.5) * head;
    [(a, b), (b, b + l), (b, b + r)]
}

fn ellipse_pts(a: Pos2, b: Pos2) -> Vec<Pos2> {
    let c = Pos2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
    let (rx, ry) = ((a.x - b.x).abs() / 2.0, (a.y - b.y).abs() / 2.0);
    (0..=96)
        .map(|i| {
            let t = i as f32 / 96.0 * std::f32::consts::TAU;
            Pos2::new(c.x + rx * t.cos(), c.y + ry * t.sin())
        })
        .collect()
}

fn draw_preview(p: &egui::Painter, s: &Shape, to_scr: &dyn Fn(Pos2) -> Pos2, scale: f32) {
    if s.pts.is_empty() {
        return;
    }
    let col = Color32::from_rgb(s.color[0], s.color[1], s.color[2]);
    let stroke = Stroke::new((s.width * scale).max(1.0), col);
    let a = s.pts[0];
    let b = *s.pts.last().unwrap();
    match s.kind {
        Tool::Pen => {
            p.add(egui::Shape::line(s.pts.iter().map(|q| to_scr(*q)).collect(), stroke));
        }
        Tool::Line => {
            p.line_segment([to_scr(a), to_scr(b)], stroke);
        }
        Tool::Arrow => {
            for (u, v) in arrow_segments(a, b, s.width) {
                p.line_segment([to_scr(u), to_scr(v)], stroke);
            }
        }
        Tool::Rect => {
            p.rect_stroke(Rect::from_two_pos(to_scr(a), to_scr(b)), 0.0, stroke);
        }
        Tool::Ellipse => {
            let pts: Vec<Pos2> = ellipse_pts(a, b).into_iter().map(|q| to_scr(q)).collect();
            p.add(egui::Shape::line(pts, stroke));
        }
        Tool::Highlight => {
            let c = Color32::from_rgba_unmultiplied(s.color[0], s.color[1], s.color[2], 97);
            p.rect_filled(Rect::from_two_pos(to_scr(a), to_scr(b)), 0.0, c);
        }
        Tool::Pixelate => {
            p.rect_stroke(Rect::from_two_pos(to_scr(a), to_scr(b)), 0.0, Stroke::new(1.5_f32, Color32::WHITE));
        }
        Tool::Text | Tool::Step | Tool::Crop => {}
    }
}

// ───────────────────────────── rasteriser ─────────────────────────────

fn render(base: &RgbaImage, shapes: &[Shape], font: &Option<FontArc>) -> RgbaImage {
    let mut img = base.clone();
    for s in shapes {
        draw_shape(&mut img, s, font);
    }
    img
}

fn draw_shape(img: &mut RgbaImage, s: &Shape, font: &Option<FontArc>) {
    if s.pts.is_empty() {
        return;
    }
    let (c, w) = (s.color, s.width);
    let a = s.pts[0];
    let b = *s.pts.last().unwrap();
    match s.kind {
        Tool::Pen => {
            if s.pts.len() == 1 {
                stamp(img, a.x, a.y, w / 2.0, c);
            }
            for p in s.pts.windows(2) {
                thick_line(img, p[0], p[1], w, c);
            }
        }
        Tool::Line => thick_line(img, a, b, w, c),
        Tool::Arrow => {
            for (u, v) in arrow_segments(a, b, w) {
                thick_line(img, u, v, w, c);
            }
        }
        Tool::Rect => {
            let r = Rect::from_two_pos(a, b);
            let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
            for i in 0..4 {
                thick_line(img, corners[i], corners[(i + 1) % 4], w, c);
            }
        }
        Tool::Ellipse => {
            for p in ellipse_pts(a, b).windows(2) {
                thick_line(img, p[0], p[1], w, c);
            }
        }
        Tool::Highlight => fill_rect(img, Rect::from_two_pos(a, b), c, 0.38),
        Tool::Pixelate => pixelate(img, Rect::from_two_pos(a, b)),
        Tool::Text => draw_text(img, font, a, &s.text, w, c),
        Tool::Step => draw_step(img, font, a, &s.text, w, c),
        Tool::Crop => {}
    }
}

fn blend(p: &mut Rgba<u8>, c: [u8; 3], a: f32) {
    for i in 0..3 {
        p.0[i] = (p.0[i] as f32 * (1.0 - a) + c[i] as f32 * a).round() as u8;
    }
    p.0[3] = 255;
}

/// Anti-aliased filled circle.
fn stamp(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, c: [u8; 3]) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let x0 = ((cx - r - 1.0).floor() as i32).max(0);
    let x1 = ((cx + r + 1.0).ceil() as i32).min(w - 1);
    let y0 = ((cy - r - 1.0).floor() as i32).max(0);
    let y1 = ((cy + r + 1.0).ceil() as i32).min(h - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let a = (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            if a > 0.0 {
                blend(img.get_pixel_mut(x as u32, y as u32), c, a);
            }
        }
    }
}

fn thick_line(img: &mut RgbaImage, a: Pos2, b: Pos2, w: f32, c: [u8; 3]) {
    let r = (w / 2.0).max(0.5);
    let len = a.distance(b);
    let n = (len / (r * 0.5).max(0.5)).ceil().max(1.0) as usize;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        stamp(img, a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, r, c);
    }
}

fn clamp_rect(img: &RgbaImage, r: Rect) -> (u32, u32, u32, u32) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    (
        r.min.x.clamp(0.0, w) as u32,
        r.min.y.clamp(0.0, h) as u32,
        r.max.x.clamp(0.0, w) as u32,
        r.max.y.clamp(0.0, h) as u32,
    )
}

fn fill_rect(img: &mut RgbaImage, r: Rect, c: [u8; 3], alpha: f32) {
    let (x0, y0, x1, y1) = clamp_rect(img, r);
    for y in y0..y1 {
        for x in x0..x1 {
            blend(img.get_pixel_mut(x, y), c, alpha);
        }
    }
}

/// "Blur" tool = mosaic, which (unlike a light blur) can't be reversed.
fn pixelate(img: &mut RgbaImage, r: Rect) {
    let (x0, y0, x1, y1) = clamp_rect(img, r);
    let block = (img.width().max(img.height()) / 120).max(10);
    let mut y = y0;
    while y < y1 {
        let by = (y + block).min(y1);
        let mut x = x0;
        while x < x1 {
            let bx = (x + block).min(x1);
            let mut sum = [0u64; 3];
            let mut n = 0u64;
            for yy in y..by {
                for xx in x..bx {
                    let p = img.get_pixel(xx, yy).0;
                    sum[0] += p[0] as u64;
                    sum[1] += p[1] as u64;
                    sum[2] += p[2] as u64;
                    n += 1;
                }
            }
            if n > 0 {
                let avg = Rgba([(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, 255]);
                for yy in y..by {
                    for xx in x..bx {
                        img.put_pixel(xx, yy, avg);
                    }
                }
            }
            x = bx;
        }
        y = by;
    }
}

/// Numbered step badge: a filled disc with the step number in the middle.
fn draw_step(img: &mut RgbaImage, font: &Option<FontArc>, pos: Pos2, num: &str, w: f32, c: [u8; 3]) {
    let r = (w * 2.0 + 6.0).max(10.0);
    stamp(img, pos.x, pos.y, r, c);
    let size = (r * 1.1).max(12.0);
    let Some(font) = font else { return };
    let sf = font.as_scaled(PxScale::from(size));
    let tw: f32 = num.chars().map(|ch| sf.h_advance(sf.glyph_id(ch))).sum();
    // Dark disc -> white digits, light disc -> black digits.
    let lum = 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
    let tc = if lum < 140.0 { [255, 255, 255] } else { [10, 10, 10] };
    // draw_text() puts the first line's top at pos.y; center the digits vertically.
    let top = Pos2::new(pos.x - tw * 0.5, pos.y - size * 0.42);
    draw_text(img, &Some(font.clone()), top, num, size, tc);
}

fn draw_text(img: &mut RgbaImage, font: &Option<FontArc>, pos: Pos2, text: &str, size: f32, c: [u8; 3]) {
    let Some(font) = font else { return };
    let scale = PxScale::from(size);
    let sf = font.as_scaled(scale);
    // Put the top of the digits/letters at pos.y. Using cap height instead of
    // ascent matters for fonts like Segoe UI whose ascent is larger than the
    // em box - otherwise text lands far below where you clicked. Cap height is
    // measured from the bounds of "H" so it works with any font file.
    let h_glyph = sf.glyph_id('H').with_scale_and_position(scale, point(0.0, 0.0));
    let cap = sf.glyph_bounds(&h_glyph).max.y.max(sf.ascent() * 0.7);
    let mut baseline = pos.y + cap;
    for line in text.lines() {
        let mut x = pos.x;
        for ch in line.chars() {
            if ch == '\r' {
                continue;
            }
            let id = sf.glyph_id(ch);
            let glyph = id.with_scale_and_position(scale, point(x, baseline));
            x += sf.h_advance(id);
            if let Some(og) = font.outline_glyph(glyph) {
                let b = og.px_bounds();
                og.draw(|gx, gy, cov| {
                    let px = b.min.x as i32 + gx as i32;
                    let py = b.min.y as i32 + gy as i32;
                    if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                        blend(img.get_pixel_mut(px as u32, py as u32), c, cov);
                    }
                });
            }
        }
        baseline += sf.height() + sf.line_gap();
    }
}

// ───────────────────────────── fonts & saving ─────────────────────────────

/// Uses the user's font file if set, otherwise the font bundled inside egui.
pub fn load_font(path: &str) -> Option<FontArc> {
    if !path.trim().is_empty() {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(f) = FontArc::try_from_vec(bytes) {
                return Some(f);
            }
        }
    }
    #[cfg(windows)]
    {
        if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
            if let Ok(f) = FontArc::try_from_vec(bytes) {
                return Some(f);
            }
        }
    }
    let defs = egui::FontDefinitions::default();
    let data = defs.font_data.get("Ubuntu-Light")?;
    FontArc::try_from_vec(data.font.to_vec()).ok()
}

pub fn save_image(img: &RgbaImage, path: &Path, quality: u8) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png").to_lowercase();
    if ext == "jpg" || ext == "jpeg" {
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let mut w = std::io::BufWriter::new(file);
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, quality.clamp(1, 100));
        image::DynamicImage::ImageRgb8(rgb).write_with_encoder(enc).map_err(|e| e.to_string())
    } else {
        img.save(path).map_err(|e| e.to_string())
    }
}
