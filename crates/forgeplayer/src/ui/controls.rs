use egui::{Color32, CornerRadius, Response, Sense, Stroke, Ui, Vec2};
use crate::config::{AspectMode, LoopMode};
use crate::player::{Player, PlayerState};

const ACCENT:    Color32 = Color32::from_rgb(74, 158, 255);
const SURFACE2:  Color32 = Color32::from_rgba_premultiplied(24, 25, 38, 215);
const DIM:       Color32 = Color32::from_gray(118);
const HDR_COLOR: Color32 = Color32::from_rgb(255, 160, 40);

pub fn show(
    ui:          &mut Ui,
    player:      &mut Player,
    seek_out:    &mut Option<f64>,
    loop_mode:   &mut LoopMode,
    aspect_mode: &mut AspectMode,
    speed_out:   &mut Option<f32>,
    audio_ok:    bool,
) {
    let screen_w = ui.ctx().screen_rect().width();
    let bg_rect  = ui.available_rect_before_wrap();

    // Gradient de fond : transparent en haut → sombre opaque en bas
    paint_gradient_bg(ui.painter(), bg_rect);

    // Ligne de séparation accent (haut du panneau)
    ui.painter().line_segment(
        [egui::pos2(bg_rect.left(), bg_rect.top()),
         egui::pos2(bg_rect.right(), bg_rect.top())],
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(74, 158, 255, 40)),
    );

    ui.style_mut().spacing.item_spacing  = Vec2::new(4.0, 2.0);
    ui.style_mut().spacing.button_padding = Vec2::new(6.0, 4.0);

    ui.vertical(|ui| {
        ui.add_space(5.0);

        // ── Ligne d'info : titre + badges ────────────────────────────────
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            if let Some(title) = player.display_title() {
                ui.label(egui::RichText::new(&title)
                    .size(11.5).color(Color32::from_gray(205)));
                if let Some(ch) = current_chapter(player) {
                    ui.label(egui::RichText::new(format!("·  {ch}"))
                        .size(11.0).color(DIM));
                }
            }
            // HDR badge
            if player.media_info.as_ref().and_then(|m| m.video.as_ref())
                .map(|v| v.hdr).unwrap_or(false)
            {
                ui.add_space(4.0);
                badge(ui, "HDR", HDR_COLOR);
            }
            // Résolution badge
            if let Some(v) = player.media_info.as_ref().and_then(|m| m.video.as_ref()) {
                let res = omni_core::Resolution { width: v.width, height: v.height };
                badge(ui, res.quality_label(), Color32::from_rgb(80, 200, 120));
            }
        });

        // ── Seek bar ──────────────────────────────────────────────────────
        ui.add_space(3.0);
        let dur = player.duration.max(1.0);
        let mut pos = player.position;
        if seek_bar(ui, &mut pos, dur, &player.chapters, screen_w).changed() {
            *seek_out = Some(pos);
        }

        // ── Ligne de contrôles ────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.add_space(8.0);

            // Chapitre précédent
            if !player.chapters.is_empty() {
                if ctrl_btn(ui, "⏮", "Chapitre précédent").clicked() { player.chapter_prev(); }
            }
            // Stop
            if ctrl_btn(ui, "⏹", "Stop").clicked() { player.stop(); }

            // Loop — icône distincte pour chaque état
            let (loop_icon, loop_color) = match loop_mode {
                LoopMode::Off => ("↩",  Color32::from_gray(65)),
                LoopMode::One => ("🔂", ACCENT),
                LoopMode::All => ("🔁", ACCENT),
            };
            if ui.add(
                egui::Button::new(egui::RichText::new(loop_icon).size(14.0).color(loop_color))
                    .min_size(Vec2::splat(28.0))
                    .fill(SURFACE2)
                    .stroke(Stroke::new(1.0, Color32::from_gray(42))),
            ).on_hover_text(format!("Répétition : {}  [L]", loop_mode.label())).clicked() {
                *loop_mode = loop_mode.next();
            }

            // Play/Pause — bouton principal mis en avant
            let play_icon = if matches!(player.state, PlayerState::Playing) { "⏸" } else { "▶" };
            if ui.add(
                egui::Button::new(egui::RichText::new(play_icon).size(16.0).color(Color32::WHITE))
                    .min_size(Vec2::new(34.0, 34.0))
                    .fill(Color32::from_rgba_premultiplied(74, 158, 255, 38))
                    .stroke(Stroke::new(1.0, Color32::from_rgba_premultiplied(74, 158, 255, 90))),
            ).on_hover_text("Lecture/Pause  [Espace]").clicked() {
                player.play_pause();
            }

            // Chapitre suivant
            if !player.chapters.is_empty() {
                if ctrl_btn(ui, "⏭", "Chapitre suivant").clicked() { player.chapter_next(); }
            }

            ui.add_space(4.0);
            if ctrl_btn(ui, "↩10", "−10 s  [←]").clicked() { player.seek_relative(-10.0); }
            if ctrl_btn(ui, "10↪", "+10 s  [→]").clicked()  { player.seek_relative(10.0); }

            ui.add_space(6.0);
            // Timestamp
            ui.label(
                egui::RichText::new(format!("{} / {}",
                    fmt_time(player.position), fmt_time(player.duration)))
                    .monospace().size(12.5).color(Color32::from_gray(218)),
            );

            // État (buffering / erreur)
            match &player.state.clone() {
                PlayerState::Buffering(b) => {
                    ui.label(egui::RichText::new(format!("⏳ {b}%")).color(Color32::YELLOW).small());
                }
                PlayerState::Error(e) => {
                    let msg: String = e.chars().take(42).collect();
                    ui.label(egui::RichText::new(format!("⚠ {msg}")).color(Color32::RED).small());
                }
                _ => {}
            }

            // ── Section droite : pistes, volume, vitesse ──────────────────
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);

                // Audio KO
                if !audio_ok {
                    ui.label(egui::RichText::new("🔇 AUDIO KO")
                        .color(Color32::from_rgb(255, 100, 100)).small());
                    ui.separator();
                }

                // Vitesse
                const SPEEDS: &[f32] = &[0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 4.0];
                let cur = player.speed();
                let spd_str = fmt_speed(cur);
                let spd_color = if (cur - 1.0).abs() < 0.01 {
                    Color32::from_gray(172)
                } else {
                    Color32::from_rgb(255, 205, 80)
                };
                ui.menu_button(
                    egui::RichText::new(&spd_str).size(11.5).color(spd_color),
                    |ui| {
                        for &s in SPEEDS {
                            let active = (cur - s).abs() < 0.01;
                            if ui.selectable_label(active, fmt_speed(s)).clicked() {
                                *speed_out = Some(s);
                                ui.close_menu();
                            }
                        }
                    },
                ).response.on_hover_text("Vitesse  [ / ]");

                // Format d'image
                let aspect_btn = ui.add(
                    egui::Button::new(egui::RichText::new(aspect_mode.label()).size(11.0))
                        .fill(SURFACE2)
                        .stroke(Stroke::new(1.0, Color32::from_gray(42))),
                ).on_hover_text("Format image  [W]");
                if aspect_btn.clicked() { *aspect_mode = aspect_mode.next(); }

                ui.separator();

                // Volume icône (muet toggle)
                let vol_icon = if player.muted { "🔇" }
                    else if player.volume == 0.0 { "🔈" }
                    else if player.volume < 0.6  { "🔉" }
                    else                         { "🔊" };
                if ctrl_btn(ui, vol_icon, "Muet  [M]").clicked() { player.toggle_mute(); }

                // Volume slider
                let mut vol = player.volume;
                if ui.add_sized([80.0, 18.0],
                    egui::Slider::new(&mut vol, 0.0..=1.5).show_value(false)
                ).changed() {
                    player.set_volume(vol);
                }

                ui.separator();

                // Piste audio (uniquement si plusieurs)
                if let Some(info) = &player.media_info {
                    if info.audio.len() > 1 {
                        let lang = info.audio.get(player.audio_track_idx)
                            .map(|a| {
                                if a.language.is_empty() || a.language == "und" {
                                    "?".to_string()
                                } else {
                                    a.language.clone()
                                }
                            })
                            .unwrap_or_default();
                        let label = format!("🎵 {lang}");
                        if ui.small_button(&label).on_hover_text("Piste audio  [A]").clicked() {
                            player.next_audio_track();
                        }
                    }
                }

                // Sous-titres
                let sub_label = match player.sub_track_idx {
                    None if player.subtitle_track.is_none() => "💬 Off".to_string(),
                    None    => "💬 Ext".to_string(),
                    Some(i) => {
                        player.media_info.as_ref()
                            .and_then(|mi| mi.subtitles.get(i))
                            .map(|s| {
                                let lang = if s.language.is_empty() { "?" } else { &s.language };
                                format!("💬 {lang}")
                            })
                            .unwrap_or_else(|| format!("💬 #{i}"))
                    }
                };
                if ui.small_button(&sub_label).on_hover_text("Sous-titres  [S]").clicked() {
                    player.next_subtitle_track();
                }
            });
        });

        ui.add_space(6.0);
    });
}

// ─── Seek bar ────────────────────────────────────────────────────────────────

fn seek_bar(
    ui:        &mut Ui,
    pos:       &mut f64,
    duration:  f64,
    chapters:  &[omni_core::probe::Chapter],
    _w:        f32,
) -> Response {
    let h       = 12.0;
    let desired = Vec2::new(ui.available_width() - 16.0, h + 12.0);
    let (rect, mut resp) = ui.allocate_exact_size(desired, Sense::click_and_drag());

    let bar = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 8.0, rect.center().y - h * 0.5),
        Vec2::new(rect.width() - 16.0, h),
    );

    if resp.clicked() || resp.dragged() {
        if let Some(mp) = resp.interact_pointer_pos() {
            let t = ((mp.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
            *pos = t as f64 * duration;
            // allocate_exact_size() ne marque jamais `changed` tout seul (ce n'est
            // pas un widget standard comme Slider) — sans ça, `.changed()() côté
            // appelant reste faux en permanence et le clic ne déclenche jamais de
            // vrai seek (seulement un redessin visuel du curseur pour cette frame,
            // qui revenait à l'ancienne position la frame suivante).
            resp.mark_changed();
        }
    }

    if ui.is_rect_visible(rect) {
        let p  = ui.painter();
        let cr = CornerRadius::from(h * 0.5);
        let t  = (*pos / duration).clamp(0.0, 1.0) as f32;
        let fw = bar.width() * t;

        // Fond de piste
        p.rect_filled(bar, cr, Color32::from_gray(36));

        // Portion jouée
        if fw > 0.0 {
            let filled = egui::Rect::from_min_size(bar.min, Vec2::new(fw, bar.height()));
            p.rect_filled(filled, cr, ACCENT);
        }

        // Marqueurs de chapitres
        for ch in chapters {
            let x = bar.left() + bar.width() * (ch.start_secs / duration) as f32;
            p.rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(x, bar.center().y),
                    Vec2::new(2.0, h + 6.0),
                ),
                CornerRadius::ZERO,
                Color32::from_rgb(255, 200, 80),
            );
        }

        // Thumb
        let thumb_x = bar.left() + fw;
        let thumb_r = if resp.hovered() || resp.dragged() { 9.0 } else { 5.0 };
        p.circle_filled(egui::pos2(thumb_x, bar.center().y), thumb_r, Color32::WHITE);
        if resp.hovered() || resp.dragged() {
            p.circle_stroke(
                egui::pos2(thumb_x, bar.center().y), thumb_r + 2.5,
                Stroke::new(1.5, Color32::from_rgba_premultiplied(74, 158, 255, 65)),
            );
        }

        // Tooltip (temps + chapitre éventuel)
        if resp.hovered() {
            if let Some(mp) = resp.hover_pos() {
                let hover_t    = ((mp.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
                let hover_secs = hover_t as f64 * duration;
                let time_str   = fmt_time(hover_secs);

                // Chapitre à cette position
                let ch_name = chapters.iter().rev()
                    .find(|c| c.start_secs <= hover_secs && !c.title.is_empty())
                    .map(|c| c.title.as_str());

                let label = if let Some(name) = ch_name {
                    format!("{name}\n{time_str}")
                } else {
                    time_str
                };

                let line_count = label.lines().count() as f32;
                let max_len   = label.lines().map(|l| l.len()).max().unwrap_or(5) as f32;
                let box_w     = max_len * 7.5 + 18.0;
                let box_h     = line_count * 18.0 + 10.0;
                let tp_x      = mp.x.clamp(bar.left() + box_w * 0.5 + 4.0,
                                           bar.right() - box_w * 0.5 - 4.0);
                let bg = egui::Rect::from_min_size(
                    egui::pos2(tp_x - box_w * 0.5, bar.top() - box_h - 8.0),
                    Vec2::new(box_w, box_h),
                );

                p.rect_filled(bg, CornerRadius::from(5.0_f32), Color32::from_black_alpha(215));
                p.rect_stroke(bg, CornerRadius::from(5.0_f32),
                    Stroke::new(1.0, Color32::from_gray(58)),
                    egui::StrokeKind::Middle);

                for (i, line) in label.lines().enumerate() {
                    let is_chapter = i == 0 && line_count > 1.0;
                    let font  = if is_chapter { egui::FontId::proportional(10.5) }
                                else          { egui::FontId::monospace(11.0) };
                    let color = if is_chapter { Color32::from_rgb(255, 205, 80) }
                                else          { Color32::WHITE };
                    p.text(
                        egui::pos2(bg.center().x, bg.top() + 5.0 + i as f32 * 18.0 + 9.0),
                        egui::Align2::CENTER_CENTER,
                        line, font, color,
                    );
                }
            }
        }
    }

    resp
}

// ─── Gradient de fond ────────────────────────────────────────────────────────

fn paint_gradient_bg(p: &egui::Painter, rect: egui::Rect) {
    let steps   = 8u32;
    let step_h  = rect.height() / steps as f32;
    for i in 0..steps {
        let frac  = i as f32 / (steps - 1) as f32;
        let alpha = (frac * frac * 225.0) as u8; // quadratique
        let y = rect.top() + i as f32 * step_h;
        let strip = egui::Rect::from_min_size(
            egui::pos2(rect.left(), y),
            egui::Vec2::new(rect.width(), step_h + 1.0),
        );
        p.rect_filled(strip, 0.0, Color32::from_rgba_premultiplied(4, 4, 12, alpha));
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn ctrl_btn(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    ui.add(
        egui::Button::new(egui::RichText::new(icon).size(14.0))
            .min_size(Vec2::splat(28.0))
            .fill(SURFACE2)
            .stroke(Stroke::new(1.0, Color32::from_gray(42))),
    ).on_hover_text(tooltip)
}

fn badge(ui: &mut Ui, text: &str, color: Color32) {
    let w = text.len() as f32 * 7.0 + 12.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 17.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::from(3.0_f32), color.linear_multiply(0.20));
        p.rect_stroke(rect, CornerRadius::from(3.0_f32),
            Stroke::new(1.0, color.linear_multiply(0.75)),
            egui::StrokeKind::Middle);
        p.text(rect.center(), egui::Align2::CENTER_CENTER, text,
            egui::FontId::monospace(9.5), color);
    }
}

fn current_chapter(player: &Player) -> Option<String> {
    if player.chapters.is_empty() { return None; }
    let pos = player.position;
    player.chapters.iter().rev()
        .find(|c| c.start_secs <= pos)
        .map(|c| c.title.clone())
}

fn fmt_speed(s: f32) -> String {
    if (s - 1.0).abs() < 0.01 { return "1×".to_string(); }
    let raw = format!("{:.2}", s);
    let trimmed = raw.trim_end_matches('0').trim_end_matches('.');
    format!("{trimmed}×")
}

pub fn fmt_time(secs: f64) -> String {
    let s   = secs.max(0.0) as u64;
    let h   = s / 3600;
    let m   = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 { format!("{h}:{m:02}:{sec:02}") } else { format!("{m}:{sec:02}") }
}
