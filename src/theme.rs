//! Windows 11 / Fluent-like look: rounded corners, blue accent, Segoe UI.

use eframe::egui::{self, Color32, Rounding, Vec2};

pub const ACCENT: Color32 = Color32::from_rgb(0, 103, 192);

/// Called every frame so it also survives a light/dark system theme switch.
pub fn apply(ctx: &egui::Context) {
    ctx.style_mut(|s| {
        let r = Rounding::same(6.0);
        let w = &mut s.visuals.widgets;
        w.noninteractive.rounding = r;
        w.inactive.rounding = r;
        w.hovered.rounding = r;
        w.active.rounding = r;
        w.open.rounding = r;
        s.visuals.window_rounding = Rounding::same(8.0);
        s.visuals.menu_rounding = Rounding::same(8.0);
        s.visuals.selection.bg_fill = ACCENT;
        s.visuals.selection.stroke.color = Color32::WHITE;
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
    });
}

pub fn install_fonts(ctx: &egui::Context) {
    #[cfg(windows)]
    {
        if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("segoe".to_owned(), egui::FontData::from_owned(bytes));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "segoe".to_owned());
            ctx.set_fonts(fonts);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = ctx;
    }
}
