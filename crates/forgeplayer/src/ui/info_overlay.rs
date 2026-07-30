use egui::{Color32, Context, RichText};
use omni_core::probe::MediaInfo;

pub struct RuntimeStats {
    pub buffered_secs:  f64,
    pub clock_secs:     f64,
    pub speed:          f32,
    pub aspect_label:   &'static str,
    pub loop_label:     &'static str,
    pub color_space:    u32,
}

const BG:      Color32 = Color32::from_black_alpha(210);
const ACCENT:  Color32 = Color32::from_rgb(74, 158, 255);
const DIM:     Color32 = Color32::from_gray(140);
const WHITE:   Color32 = Color32::WHITE;

/// Overlay info technique (touche I). Affiché en haut-droite de l'écran.
pub fn show(ctx: &Context, info: &MediaInfo, is_hdr: bool, stats: Option<&RuntimeStats>) {
    let screen = ctx.screen_rect();

    egui::Area::new(egui::Id::new("info_overlay"))
        .fixed_pos(egui::pos2(screen.right() - 290.0, screen.top() + 48.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(BG)
                .corner_radius(egui::CornerRadius::from(8.0_f32))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(266.0);
                    ui.spacing_mut().item_spacing.y = 3.0;

                    ui.label(RichText::new("ℹ  Informations média").strong().color(ACCENT).size(12.5));
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(3.0);

                    // Fichier
                    let name = std::path::Path::new(&info.path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| info.path.clone());
                    row(ui, "Fichier", &truncate(&name, 28));
                    row(ui, "Conteneur", &info.format_name);
                    if info.duration_secs > 0.0 {
                        row(ui, "Durée", &fmt_duration(info.duration_secs));
                    }
                    if info.bit_rate > 0 {
                        row(ui, "Débit total", &fmt_bitrate(info.bit_rate));
                    }

                    // Vidéo
                    if let Some(v) = &info.video {
                        ui.add_space(6.0);
                        ui.label(RichText::new("▶  Vidéo").color(ACCENT).size(11.0));
                        row(ui, "Codec", &v.codec_name.to_uppercase());
                        row(ui, "Résolution", &format!("{}×{}", v.width, v.height));
                        let res = omni_core::Resolution { width: v.width, height: v.height };
                        row(ui, "Qualité", res.quality_label());
                        if v.fps > 0.0 {
                            row(ui, "FPS", &format!("{:.3}", v.fps));
                        }
                        if v.bit_rate > 0 {
                            row(ui, "Débit vidéo", &fmt_bitrate(v.bit_rate));
                        }
                        if !v.color_space.is_empty() && v.color_space != "Unspecified" {
                            row(ui, "Espace couleur", &v.color_space);
                        }
                        if is_hdr || v.hdr {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("HDR").size(11.0).color(DIM));
                                ui.label(RichText::new("● HDR10")
                                    .color(Color32::from_rgb(255, 160, 40)).size(11.0));
                            });
                        }
                    }

                    // Audio
                    if !info.audio.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new("🔊  Audio").color(ACCENT).size(11.0));
                        for (i, a) in info.audio.iter().enumerate() {
                            let lang = if a.language.is_empty() || a.language == "und" {
                                "inconnu".to_string()
                            } else { a.language.clone() };
                            row(ui, &format!("Piste {}", i + 1),
                                &format!("{} · {}", a.codec_name.to_uppercase(), lang));
                        }
                    }

                    // Sous-titres
                    if !info.subtitles.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new("💬  Sous-titres").color(ACCENT).size(11.0));
                        for s in &info.subtitles {
                            let lang = if s.language.is_empty() { "und" } else { &s.language };
                            let label = if s.title.is_empty() {
                                lang.to_string()
                            } else {
                                format!("{} ({})", s.title, lang)
                            };
                            row(ui, &s.codec, &truncate(&label, 22));
                        }
                    }

                    // Stats runtime
                    if let Some(s) = stats {
                        ui.add_space(6.0);
                        ui.label(RichText::new("⚡  Runtime").color(ACCENT).size(11.0));
                        let cs_name = match s.color_space {
                            0 => "BT.601",
                            2 => "BT.2020",
                            _ => "BT.709",
                        };
                        row(ui, "Espace couleur (actif)", cs_name);
                        row(ui, "Vitesse", &format!("{}×", s.speed));
                        row(ui, "Format image", s.aspect_label);
                        row(ui, "Répétition", s.loop_label);
                        if s.buffered_secs > 0.0 {
                            row(ui, "Buffer audio", &format!("{:.2} s", s.buffered_secs));
                        }
                        row(ui, "Position clock", &format!("{:.3} s", s.clock_secs));
                    }

                    ui.add_space(4.0);
                    ui.label(RichText::new("[ I ] pour fermer").color(DIM).size(10.0));
                });
        });
}

fn row(ui: &mut egui::Ui, key: &str, val: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(key).size(11.0).color(DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(val).size(11.0).color(WHITE).monospace());
        });
    });
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max { return s.to_string(); }
    let end: String = s.chars().take(max - 1).collect();
    format!("{}…", end)
}

fn fmt_duration(secs: f64) -> String {
    let s = secs as u64;
    let h = s / 3600; let m = (s % 3600) / 60; let sec = s % 60;
    if h > 0 { format!("{h}:{m:02}:{sec:02}") } else { format!("{m}:{sec:02}") }
}

fn fmt_bitrate(bits: i64) -> String {
    if bits <= 0 { return "—".to_string(); }
    if bits >= 1_000_000 { format!("{:.1} Mb/s", bits as f64 / 1_000_000.0) }
    else { format!("{:.0} kb/s", bits as f64 / 1_000.0) }
}
