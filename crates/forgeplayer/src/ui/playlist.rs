use egui::{Color32, RichText, ScrollArea, Ui, Vec2};

const ACCENT: Color32 = Color32::from_rgb(80, 140, 255);

/// Action demandée par l'utilisateur via l'en-tête du panneau — l'app gère
/// les dialogues fichiers (rfd / file browser interne) correspondants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistAction {
    None,
    Add,
    Save,
    Load,
}

/// Panneau playlist latéral — drag & drop, réorganisation, suppression.
/// Retourne l'action d'en-tête demandée (Ajouter/Enregistrer/Charger).
pub fn show<F>(
    ui:           &mut Ui,
    items:        &mut Vec<String>,
    current_idx:  &mut Option<usize>,
    mut on_play:  F,
) -> PlaylistAction where
    F: FnMut(String),
{
    let mut action = PlaylistAction::None;

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 2.0);

        // En-tête
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("≡  Playlist").strong().size(14.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("✕ Vider").clicked() {
                    items.clear();
                    *current_idx = None;
                }
                if ui.small_button("+ Ajouter").on_hover_text("Ajouter un fichier").clicked() {
                    action = PlaylistAction::Add;
                }
                if !items.is_empty()
                    && ui.small_button("💾").on_hover_text("Enregistrer la playlist (.m3u8)").clicked()
                {
                    action = PlaylistAction::Save;
                }
                if ui.small_button("📂").on_hover_text("Charger une playlist (.m3u/.m3u8)").clicked() {
                    action = PlaylistAction::Load;
                }
            });
        });
        ui.separator();

        if items.is_empty() {
            ui.add_space(20.0);
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("Playlist vide\nGlissez des fichiers ici")
                        .color(Color32::from_gray(110))
                        .size(12.0),
                );
            });
            return;
        }

        // Accepte le drag & drop externe de fichiers
        handle_drop(ui, items);

        let mut to_remove: Option<usize> = None;
        let mut to_move:   Option<(usize, usize)> = None;

        ScrollArea::vertical().show(ui, |ui| {
            for (idx, path) in items.iter().enumerate() {
                let is_current = Some(idx) == *current_idx;
                let name = short_name(path);

                let resp = ui.add(
                    egui::Button::new(
                        RichText::new(format!(
                            "{} {}",
                            if is_current { "▶" } else { "  " },
                            name
                        ))
                        .size(12.5)
                        .color(if is_current { ACCENT } else { Color32::from_gray(210) }),
                    )
                    .min_size(Vec2::new(ui.available_width(), 28.0))
                    .fill(if is_current {
                        Color32::from_rgb(20, 30, 55)
                    } else {
                        Color32::TRANSPARENT
                    }),
                )
                .on_hover_text(path);

                if resp.double_clicked() {
                    *current_idx = Some(idx);
                    on_play(path.clone());
                }

                resp.context_menu(|ui| {
                    if ui.button("▶ Lire").clicked() {
                        *current_idx = Some(idx);
                        on_play(path.clone());
                        ui.close_menu();
                    }
                    ui.separator();
                    if idx > 0 && ui.button("↑ Monter").clicked() {
                        to_move = Some((idx, idx - 1));
                        ui.close_menu();
                    }
                    if idx + 1 < items.len() && ui.button("↓ Descendre").clicked() {
                        to_move = Some((idx, idx + 1));
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("🗑 Supprimer").clicked() {
                        to_remove = Some(idx);
                        ui.close_menu();
                    }
                });
            }
        });

        // Applique les mutations après le rendu
        if let Some(idx) = to_remove {
            items.remove(idx);
            match current_idx {
                Some(c) if *c == idx => *current_idx = None,
                Some(c) if *c > idx  => *current_idx = Some(*c - 1),
                _ => {}
            }
        }
        if let Some((from, to)) = to_move {
            items.swap(from, to);
            if let Some(c) = current_idx {
                if *c == from { *current_idx = Some(to); }
                else if *c == to { *current_idx = Some(from); }
            }
        }
    });

    action
}

/// Accepte les fichiers glissés depuis l'explorateur Windows.
fn handle_drop(ui: &mut Ui, items: &mut Vec<String>) {
    if !ui.ctx().input(|i| i.raw.hovered_files.is_empty()) {
        // Surbrillance pendant le survol
        let rect = ui.ctx().screen_rect();
        ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop_target"),
        ))
        .rect_stroke(rect, 4.0, egui::Stroke::new(2.0, ACCENT), egui::StrokeKind::Middle);
    }

    let dropped: Vec<String> = ui.ctx().input(|i| {
        i.raw
            .dropped_files
            .iter()
            .filter_map(|f| f.path.as_ref().map(|p| p.to_string_lossy().to_string()))
            .collect()
    });

    for path in dropped {
        if !items.contains(&path) {
            items.push(path);
        }
    }
}

fn short_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}
