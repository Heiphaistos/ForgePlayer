use egui::{Color32, CornerRadius, Pos2, Rect, Sense, TextureHandle, Ui, Vec2};

pub struct ImageViewer {
    zoom: f32,
    pan:  Vec2,
}

impl Default for ImageViewer {
    fn default() -> Self { Self { zoom: 1.0, pan: Vec2::ZERO } }
}

impl ImageViewer {
    pub fn reset(&mut self) {
        self.zoom = 1.0;
        self.pan  = Vec2::ZERO;
    }

    pub fn show(&mut self, ui: &mut Ui, tex: &TextureHandle, img_w: u32, img_h: u32) {
        let available = ui.available_rect_before_wrap();
        ui.painter().rect_filled(available, 0.0, Color32::from_rgb(8, 8, 12));

        let fit_scale = (available.width() / img_w as f32)
            .min(available.height() / img_h as f32)
            .min(1.0);

        let display_w = img_w as f32 * fit_scale * self.zoom;
        let display_h = img_h as f32 * fit_scale * self.zoom;

        let center  = available.center();
        let img_rect = Rect::from_center_size(
            Pos2::new(center.x + self.pan.x, center.y + self.pan.y),
            Vec2::new(display_w, display_h),
        );

        ui.painter().image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        // ── Interactions ──────────────────────────────────────────────────
        let resp = ui.allocate_rect(available, Sense::click_and_drag());

        // Double-clic → reset zoom + pan
        if resp.double_clicked() {
            self.zoom = 1.0;
            self.pan  = Vec2::ZERO;
        }

        // Scroll → zoom vers le pointeur
        let scroll = ui.ctx().input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let old_zoom = self.zoom;
            self.zoom    = (self.zoom * (1.0 + scroll * 0.002)).clamp(0.08, 24.0);
            if let Some(ptr) = ui.ctx().input(|i| i.pointer.hover_pos()) {
                let d = ptr - (center + self.pan);
                self.pan += d * (1.0 - self.zoom / old_zoom);
            }
        }

        // Drag → pan
        if resp.dragged() {
            self.pan += resp.drag_delta();
        }

        // Clavier
        ui.ctx().input(|i| {
            if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
                self.zoom = (self.zoom * 1.2).min(24.0);
            }
            if i.key_pressed(egui::Key::Minus) {
                self.zoom = (self.zoom / 1.2).max(0.08);
            }
            if i.key_pressed(egui::Key::Num0) {
                self.zoom = 1.0;
                self.pan  = Vec2::ZERO;
            }
        });

        // ── Indicateurs superposés ────────────────────────────────────────
        let zoom_pct  = (self.zoom * fit_scale * 100.0).round() as u32;
        let info_text = format!("{}×{}  ·  {}%", img_w, img_h, zoom_pct);
        let pill_w    = info_text.len() as f32 * 6.8 + 22.0;
        let pill_pos  = egui::pos2(available.center().x, available.bottom() - 16.0);
        let pill_rect = Rect::from_center_size(pill_pos, Vec2::new(pill_w, 22.0));

        ui.painter().rect_filled(
            pill_rect, CornerRadius::from(11.0_f32),
            Color32::from_rgba_premultiplied(0, 0, 0, 150),
        );
        ui.painter().text(
            pill_pos, egui::Align2::CENTER_CENTER, &info_text,
            egui::FontId::proportional(11.0),
            Color32::from_rgba_unmultiplied(205, 205, 205, 210),
        );

        // Indication double-clic quand zoomé
        if (self.zoom - 1.0).abs() > 0.05 {
            let tip_pos  = egui::pos2(available.right() - 8.0, available.bottom() - 16.0);
            let tip_text = "Double-clic : réinitialiser";
            let tip_w    = tip_text.len() as f32 * 6.5 + 16.0;
            let tip_rect = Rect::from_center_size(
                egui::pos2(tip_pos.x - tip_w * 0.5, tip_pos.y),
                Vec2::new(tip_w, 20.0),
            );
            ui.painter().rect_filled(tip_rect, CornerRadius::from(10.0_f32),
                Color32::from_rgba_premultiplied(0, 0, 0, 120));
            ui.painter().text(
                egui::pos2(tip_rect.center().x, tip_pos.y),
                egui::Align2::CENTER_CENTER,
                tip_text,
                egui::FontId::proportional(10.0),
                Color32::from_rgba_unmultiplied(160, 160, 160, 180),
            );
        }
    }
}
