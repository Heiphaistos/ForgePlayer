use egui::{Color32, ComboBox, Context, DragValue, RichText, Slider, Window};
use crate::config::AppConfig;

pub fn show(ctx: &Context, open: &mut bool, cfg: &mut AppConfig) {
    let mut changed = false;

    Window::new("⚙  Paramètres")
        .open(open)
        .resizable(true)
        .default_size([500.0, 560.0])
        .min_size([420.0, 300.0])
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {

                // ── Vidéo ─────────────────────────────────────────────────────
                section(ui, "🎬 Vidéo & Décodage");

                ui.horizontal(|ui| {
                    ui.label("Accélération matérielle");
                    ComboBox::from_id_salt("hw_accel")
                        .selected_text(&cfg.hw_accel)
                        .show_ui(ui, |ui| {
                            for opt in &["auto", "dxva2", "d3d11va", "none"] {
                                if ui.selectable_label(cfg.hw_accel == *opt, *opt).clicked() {
                                    cfg.hw_accel = opt.to_string();
                                    changed = true;
                                }
                            }
                        });
                });

                ui.add_space(4.0);
                ui.label(RichText::new("Tone mapping HDR").size(12.0));
                ui.horizontal(|ui| {
                    for (mode, label) in [(0u32, "Reinhard"), (1, "ACES"), (2, "Hable")] {
                        if ui.radio(cfg.tonemap_mode == mode, label).clicked() {
                            cfg.tonemap_mode = mode;
                            changed = true;
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Luminance max (nits)");
                    let drag = DragValue::new(&mut cfg.max_luminance).range(100.0..=10000.0).speed(50.0);
                    if ui.add(drag).changed() { changed = true; }
                });

                ui.add_space(8.0);
                ui.separator();

                // ── Audio ─────────────────────────────────────────────────────
                section(ui, "🔊 Audio");

                ui.horizontal(|ui| {
                    ui.label("Volume par défaut");
                    let sl = Slider::new(&mut cfg.volume, 0.0..=1.5).show_value(true);
                    if ui.add(sl).changed() { changed = true; }
                });

                ui.add_space(8.0);
                ui.separator();

                // ── Sous-titres ───────────────────────────────────────────────
                section(ui, "💬 Sous-titres");

                ui.horizontal(|ui| {
                    ui.label("Langue préférée");
                    ComboBox::from_id_salt("sub_lang")
                        .selected_text(&cfg.subtitle_lang)
                        .show_ui(ui, |ui| {
                            for lang in &["fr", "en", "es", "de", "it", "pt", "ja", "ko", "zh"] {
                                if ui.selectable_label(cfg.subtitle_lang == *lang, *lang).clicked() {
                                    cfg.subtitle_lang = lang.to_string();
                                    changed = true;
                                }
                            }
                        });
                });

                ui.add_space(8.0);
                ui.separator();

                // ── Services Go ───────────────────────────────────────────────
                section(ui, "🌐 Services Go");

                ui.horizontal(|ui| {
                    ui.label("Port sous-titres");
                    let d = DragValue::new(&mut cfg.subtitle_service_port).range(1024..=65535);
                    if ui.add(d).changed() { changed = true; }
                });
                ui.horizontal(|ui| {
                    ui.label("Port indexeur médias");
                    let d = DragValue::new(&mut cfg.media_indexer_port).range(1024..=65535);
                    if ui.add(d).changed() { changed = true; }
                });

                ui.add_space(8.0);
                ui.separator();

                // ── Bibliothèque ──────────────────────────────────────────────
                section(ui, "📁 Bibliothèque Médias");

                let mut to_remove: Option<usize> = None;
                for (i, dir) in cfg.media_library.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(dir).monospace().size(11.0).color(Color32::from_gray(200)));
                        if ui.small_button("✕").on_hover_text("Retirer").clicked() {
                            to_remove = Some(i);
                        }
                    });
                }
                if let Some(i) = to_remove {
                    cfg.media_library.remove(i);
                    changed = true;
                }

                if ui.button("+ Ajouter un dossier…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        let dir = path.to_string_lossy().to_string();
                        if !cfg.media_library.contains(&dir) {
                            cfg.media_library.push(dir);
                            changed = true;
                        }
                    }
                }

                ui.add_space(12.0);

                // ── Actions ───────────────────────────────────────────────────
                ui.horizontal(|ui| {
                    if ui.button("💾 Sauvegarder").clicked() {
                        cfg.save();
                        changed = false;
                    }
                    if changed {
                        ui.label(
                            RichText::new("● Non sauvegardé")
                                .color(Color32::YELLOW)
                                .small(),
                        );
                    }
                });
            });
        });
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).strong().size(13.0).color(Color32::from_rgb(80, 140, 255)));
    ui.add_space(4.0);
}
