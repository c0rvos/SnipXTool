//! Video editor panel: scrub to preview frames, trim start/end, mute,
//! export MP4 or GIF, or grab a frame and annotate it in the image editor.

use eframe::egui;
use std::path::PathBuf;

#[derive(Clone, Copy)]
pub enum VideoAction {
    Preview,
    Export,
    Gif,
    GrabFrame,
    Play,
}

pub struct VideoEditor {
    pub path: PathBuf,
    pub duration: f64,
    pub start: f64,
    pub end: f64,
    pub pos: f64,
    pub mute: bool,
    pub tex: Option<egui::TextureHandle>,
    pub busy: bool,
}

impl VideoEditor {
    pub fn new(path: PathBuf) -> Self {
        Self { path, duration: 0.0, start: 0.0, end: 0.0, pos: 0.0, mute: false, tex: None, busy: false }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<VideoAction> {
        let mut act: Option<VideoAction> = None;
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        ui.heading(name);
        ui.add_space(6.0);

        // preview frame
        let max_h = (ui.available_height() - 200.0).max(160.0);
        let max_w = ui.available_width();
        if let Some(tex) = &self.tex {
            let sz = tex.size_vec2();
            let s = (max_w / sz.x).min(max_h / sz.y).min(1.0);
            ui.vertical_centered(|ui| {
                ui.image((tex.id(), sz * s));
            });
        } else {
            ui.label("Loading preview...");
        }
        ui.add_space(8.0);

        let dur = self.duration.max(0.1);
        ui.horizontal(|ui| {
            ui.label("Position");
            let r = ui.add(egui::Slider::new(&mut self.pos, 0.0..=dur).suffix(" s").fixed_decimals(2));
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                act = Some(VideoAction::Preview);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Trim start");
            ui.add(egui::Slider::new(&mut self.start, 0.0..=dur).suffix(" s").fixed_decimals(2));
            if ui.button("= current").clicked() {
                self.start = self.pos;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Trim end  ");
            ui.add(egui::Slider::new(&mut self.end, 0.0..=dur).suffix(" s").fixed_decimals(2));
            if ui.button("= current").clicked() {
                self.end = self.pos;
            }
        });
        self.end = self.end.clamp(0.0, dur);
        self.start = self.start.clamp(0.0, (self.end - 0.1).max(0.0));

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.mute, "Mute audio");
            ui.label(format!("Clip length: {:.1} s", (self.end - self.start).max(0.0)));
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!self.busy, |ui| {
                if ui.button("Export trimmed MP4").clicked() {
                    act = Some(VideoAction::Export);
                }
                if ui.button("Export GIF").clicked() {
                    act = Some(VideoAction::Gif);
                }
                if ui.button("Grab frame and edit as image").clicked() {
                    act = Some(VideoAction::GrabFrame);
                }
            });
            if ui.button("Play in system player").clicked() {
                act = Some(VideoAction::Play);
            }
            if self.busy {
                ui.spinner();
                ui.label("Working...");
            }
        });
        act
    }
}
