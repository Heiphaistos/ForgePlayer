use egui::{Color32, CornerRadius, Rect, Sense, TextureHandle, Ui, Vec2};
use eframe::egui_wgpu;
use crate::config::AspectMode;
use crate::player::{Player, PlayerState};
use crate::video_callback::SharedFrame;
use crate::ui::image_viewer::ImageViewer;

const ACCENT:       Color32 = Color32::from_rgb(74, 158, 255);
const SUBTITLE_BG:  Color32 = Color32::from_rgba_premultiplied(5, 5, 16, 205);

/// Retourne `true` si l'utilisateur a double-cliqué (demande bascule plein écran).
pub fn show(
    ui:          &mut Ui,
    player:      &Player,
    video_frame: SharedFrame,
    osd:         Option<&str>,
    image_tex:   Option<&TextureHandle>,
    img_viewer:  &mut ImageViewer,
    aspect_mode: &AspectMode,
    color_space: u32,
    is_hdr:        bool,
    tonemap_mode:  u32,
    max_luminance: f32,
) -> bool {
    let available = ui.available_rect_before_wrap();

    // Mode image
    if player.is_image_mode() {
        if let (Some(tex), Some(img)) = (image_tex, &player.image_frame) {
            img_viewer.show(ui, tex, img.width, img.height);
        } else {
            draw_idle_screen(ui, available);
        }
        return false;
    }

    // États non-vidéo
    match &player.state {
        PlayerState::Idle    => { draw_idle_screen(ui, available); return false; }
        PlayerState::Loading => { draw_loading(ui, available);     return false; }
        PlayerState::Error(e)=> {
            let e = e.clone();
            draw_error(ui, available, &e);
            return false;
        }
        _ => {}
    }

    // ── Vidéo ───────────────────────────────────────────────────────────────
    let video_rect = player.media_info.as_ref()
        .and_then(|m| m.video.as_ref())
        .map(|v| compute_video_rect(available, v.width, v.height, aspect_mode))
        .unwrap_or(available);

    draw_video(ui, available, video_rect, video_frame, color_space, is_hdr, tonemap_mode, max_luminance);

    // Zone d'interaction (double-clic = plein écran)
    let vid_resp = ui.allocate_rect(available, Sense::click());
    let toggle_fs = vid_resp.double_clicked();

    // Buffering
    if let PlayerState::Buffering(pct) = &player.state {
        draw_buffering_overlay(ui, available, *pct);
    }

    // Sous-titres
    if let Some(text) = &player.current_subtitle {
        draw_subtitle(ui, available, text);
    }

    // OSD
    if let Some(text) = osd {
        draw_osd(ui, available, text);
    }

    toggle_fs
}

// ─── Calcul du rectangle vidéo ──────────────────────────────────────────────

fn compute_video_rect(available: Rect, video_w: u32, video_h: u32, mode: &AspectMode) -> Rect {
    if video_w == 0 || video_h == 0 { return available; }
    match mode {
        AspectMode::Stretch => available,
        AspectMode::Fit => {
            let scale = (available.width()  / video_w as f32)
                .min(available.height() / video_h as f32);
            let w = video_w as f32 * scale;
            let h = video_h as f32 * scale;
            Rect::from_center_size(available.center(), Vec2::new(w, h))
        }
        AspectMode::Fill => {
            let scale = (available.width()  / video_w as f32)
                .max(available.height() / video_h as f32);
            let w = video_w as f32 * scale;
            let h = video_h as f32 * scale;
            Rect::from_center_size(available.center(), Vec2::new(w, h))
        }
    }
}

// ─── Rendu vidéo ─────────────────────────────────────────────────────────────

fn draw_video(
    ui: &mut Ui, available: Rect, video_rect: Rect, video_frame: SharedFrame, color_space: u32,
    is_hdr: bool, tonemap_mode: u32, max_luminance: f32,
) {
    ui.painter().rect_filled(available, 0.0, Color32::BLACK);
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(
        video_rect,
        crate::video_callback::VideoPaintCallback {
            frame: video_frame, color_space, is_hdr, tonemap_mode, max_luminance,
        },
    ));
}

// ─── Sous-titres (avec contour 5 directions) ─────────────────────────────────

fn draw_subtitle(ui: &mut Ui, rect: Rect, text: &str) {
    let painter = ui.painter();
    let font    = egui::FontId::proportional(19.0);
    let lines: Vec<&str> = text.lines().collect();
    let line_h  = 28.0;
    let pad     = Vec2::new(20.0, 10.0);
    let max_chars = lines.iter().map(|l| l.len()).max().unwrap_or(1);
    let box_w   = (max_chars as f32 * 10.8 + pad.x * 2.0).max(80.0).min(rect.width() * 0.88);
    let box_h   = lines.len() as f32 * line_h + pad.y * 2.0;
    let box_min = egui::pos2(
        rect.center().x - box_w * 0.5,
        rect.bottom() - box_h - 115.0,
    );
    let box_rect = Rect::from_min_size(box_min, Vec2::new(box_w, box_h));

    painter.rect_filled(box_rect, CornerRadius::from(8.0_f32), SUBTITLE_BG);
    painter.rect_stroke(box_rect, CornerRadius::from(8.0_f32),
        egui::Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 255, 255, 18)),
        egui::StrokeKind::Middle);

    for (i, line) in lines.iter().enumerate() {
        let cy = box_min.y + pad.y + i as f32 * line_h + line_h * 0.5;
        let cx = rect.center().x;
        // Contour 5 directions (lisibilité maximale)
        for (dx, dy) in &[(-1.5f32, 0.0), (1.5, 0.0), (0.0, -1.5), (0.0, 1.5), (0.0, 2.5)] {
            painter.text(
                egui::pos2(cx + dx, cy + dy),
                egui::Align2::CENTER_CENTER,
                *line, font.clone(),
                Color32::from_black_alpha(190),
            );
        }
        // Texte blanc
        painter.text(
            egui::pos2(cx, cy),
            egui::Align2::CENTER_CENTER,
            *line, font.clone(), Color32::WHITE,
        );
    }
}

// ─── OSD ─────────────────────────────────────────────────────────────────────

fn draw_osd(ui: &mut Ui, rect: Rect, text: &str) {
    let p        = ui.painter();
    let lines: Vec<&str> = text.lines().collect();
    let max_len  = lines.iter().map(|l| l.len()).max().unwrap_or(1) as f32;
    let sz       = Vec2::new(max_len * 9.0 + 26.0, lines.len() as f32 * 20.0 + 18.0);
    let center   = egui::pos2(rect.center().x, rect.top() + sz.y * 0.5 + 44.0);
    let bg       = Rect::from_center_size(center, sz);

    p.rect_filled(bg, CornerRadius::from(8.0_f32),
        Color32::from_rgba_premultiplied(8, 8, 22, 225));
    p.rect_stroke(bg, CornerRadius::from(8.0_f32),
        egui::Stroke::new(1.0, Color32::from_rgba_premultiplied(74, 158, 255, 65)),
        egui::StrokeKind::Middle);

    for (i, line) in lines.iter().enumerate() {
        p.text(
            egui::pos2(center.x, bg.top() + 9.0 + i as f32 * 20.0 + 10.0),
            egui::Align2::CENTER_CENTER,
            *line,
            egui::FontId::proportional(14.5),
            Color32::WHITE,
        );
    }
}

// ─── Écran d'accueil ─────────────────────────────────────────────────────────

fn draw_idle_screen(ui: &mut Ui, rect: Rect) {
    let p = ui.painter();
    p.rect_filled(rect, 0.0, Color32::from_rgb(8, 8, 14));

    let c = egui::pos2(rect.center().x, rect.top() + (rect.height() - 100.0) * 0.40);

    // Anneaux de halo derrière le bouton play
    for (r, a) in &[(88.0f32, 6u8), (72.0, 12), (60.0, 20)] {
        p.circle_stroke(c, *r,
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(74, 158, 255, *a)));
    }

    // Disque play
    p.circle_filled(c, 48.0, Color32::from_rgba_premultiplied(74, 158, 255, 20));
    p.circle_stroke(c, 48.0,
        egui::Stroke::new(1.5, Color32::from_rgba_unmultiplied(74, 158, 255, 70)));
    p.text(c, egui::Align2::CENTER_CENTER,
        "▶", egui::FontId::proportional(44.0), ACCENT);

    // Titre + accroche
    p.text(egui::pos2(c.x, c.y + 70.0), egui::Align2::CENTER_CENTER,
        "OmniPlayer", egui::FontId::proportional(22.0), Color32::from_gray(232));
    p.text(egui::pos2(c.x, c.y + 91.0), egui::Align2::CENTER_CENTER,
        "Glissez un fichier  ·  Ctrl+O  ·  Ctrl+L",
        egui::FontId::proportional(11.0), Color32::from_gray(100));

    // Diviseur
    let div_y = c.y + 108.0;
    p.line_segment(
        [egui::pos2(c.x - 118.0, div_y), egui::pos2(c.x + 118.0, div_y)],
        egui::Stroke::new(0.5, Color32::from_rgba_unmultiplied(74, 158, 255, 32)),
    );

    // Raccourcis clavier
    let shortcuts = [
        ("Espace",    "Lecture / Pause"),
        ("← / →",     "±10 s  ·  Shift ±60 s  ·  Alt ±1 s"),
        ("↑ / ↓",     "Volume ±10 %"),
        ("[ / ]",     "Vitesse −/+"),
        ("W",         "Aspect  Fit / Fill / Stretch"),
        ("L",         "Répétition  Off / ×1 / All"),
        ("F",         "Plein écran  (double-clic sur la vidéo)"),
        ("M / S / A", "Muet  |  Sous-titres  |  Piste audio"),
        ("I",         "Infos média"),
        ("Ctrl+O",    "Ouvrir fichier"),
        ("Ctrl+L",    "Ouvrir URL"),
        ("Ctrl+P",    "Playlist"),
    ];

    let mut y = div_y + 18.0;
    for (key, action) in &shortcuts {
        p.text(egui::pos2(c.x - 96.0, y), egui::Align2::RIGHT_CENTER,
            *key, egui::FontId::monospace(10.0), ACCENT);
        p.text(egui::pos2(c.x - 82.0, y), egui::Align2::LEFT_CENTER,
            *action, egui::FontId::proportional(10.5), Color32::from_gray(138));
        y += 16.5;
    }
}

// ─── Chargement (spinner rotatif) ────────────────────────────────────────────

fn draw_loading(ui: &mut Ui, rect: Rect) {
    ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(8, 8, 14));
    let t  = ui.ctx().input(|i| i.time) as f32;
    let c  = rect.center();
    let r  = 24.0;
    let p  = ui.painter();
    let n  = 10u32;

    for i in 0..n {
        let frac  = i as f32 / n as f32;
        let angle = t * std::f32::consts::TAU * 1.1 + frac * std::f32::consts::TAU;
        let alpha = (frac * 230.0) as u8;
        let dot_r = 2.2 + frac * 1.8;
        let pos   = egui::pos2(c.x + angle.cos() * r, c.y + angle.sin() * r);
        p.circle_filled(pos, dot_r, Color32::from_rgba_unmultiplied(74, 158, 255, alpha));
    }

    p.text(
        egui::pos2(c.x, c.y + r + 22.0),
        egui::Align2::CENTER_CENTER,
        "Chargement…",
        egui::FontId::proportional(13.0),
        Color32::from_gray(148),
    );
    ui.ctx().request_repaint();
}

// ─── Overlay buffering ────────────────────────────────────────────────────────

fn draw_buffering_overlay(ui: &mut Ui, rect: Rect, pct: u8) {
    let bar_w = rect.width().min(400.0) * 0.55;
    let bar_y = rect.bottom() - 140.0;
    let bg_r  = Rect::from_min_size(
        egui::pos2(rect.center().x - bar_w * 0.5, bar_y),
        Vec2::new(bar_w, 4.0),
    );
    let fil_r = Rect::from_min_size(bg_r.min,
        Vec2::new(bar_w * pct as f32 / 100.0, 4.0));
    let p = ui.painter();
    p.rect_filled(bg_r,  CornerRadius::from(2.0_f32), Color32::from_gray(38));
    p.rect_filled(fil_r, CornerRadius::from(2.0_f32), ACCENT);
    p.text(
        egui::pos2(rect.center().x, bar_y - 18.0),
        egui::Align2::CENTER_CENTER,
        format!("Chargement… {pct}%"),
        egui::FontId::proportional(12.5),
        Color32::from_gray(185),
    );
}

// ─── Erreur ──────────────────────────────────────────────────────────────────

fn draw_error(ui: &mut Ui, rect: Rect, msg: &str) {
    let p = ui.painter();
    p.rect_filled(rect, 0.0, Color32::from_rgb(14, 5, 5));
    let c = rect.center();
    // Halo rouge
    p.circle_filled(c, 42.0, Color32::from_rgba_premultiplied(255, 40, 40, 12));
    p.circle_stroke(c, 42.0, egui::Stroke::new(1.5, Color32::from_rgba_unmultiplied(255, 75, 75, 55)));
    p.text(egui::pos2(c.x, c.y - 24.0), egui::Align2::CENTER_CENTER,
        "⚠", egui::FontId::proportional(34.0), Color32::from_rgb(255, 75, 75));
    p.text(egui::pos2(c.x, c.y + 20.0), egui::Align2::CENTER_CENTER,
        "Erreur de lecture",
        egui::FontId::proportional(18.0), Color32::from_rgb(255, 90, 90));
    p.text(egui::pos2(c.x, c.y + 46.0), egui::Align2::CENTER_CENTER,
        msg, egui::FontId::proportional(12.0), Color32::from_gray(162));
    p.text(egui::pos2(c.x, c.y + 68.0), egui::Align2::CENTER_CENTER,
        "Ctrl+O pour ouvrir un autre fichier",
        egui::FontId::proportional(11.0), Color32::from_gray(100));
}
