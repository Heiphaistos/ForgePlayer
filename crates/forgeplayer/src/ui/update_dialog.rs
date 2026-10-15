use crate::updater::{self, Manifest, Status};
use egui::{Context, ProgressBar, Window};
use std::path::PathBuf;

pub enum Action {
    None,
    Check,
    Download(Manifest),
    /// Ferme l'appli puis lance l'installateur.
    InstallNow(PathBuf),
    /// Lance l'installateur quand l'utilisateur quittera de lui-même.
    InstallOnExit(PathBuf),
}

/// Fenêtre de mise à jour. `playing` : une lecture est en cours (on ne la coupe jamais sans accord).
pub fn show(ctx: &Context, open: &mut bool, status: &Status, playing: bool, installed: bool) -> Action {
    let mut action = Action::None;
    let mut close = false;
    Window::new("🔄  Mise à jour")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(340.0);
            match status {
                Status::Idle | Status::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Recherche d'une mise à jour…");
                    });
                }
                Status::UpToDate => {
                    ui.label(format!("ForgePlayer est à jour (version {}).", env!("CARGO_PKG_VERSION")));
                    if ui.button("Fermer").clicked() { close = true; }
                }
                Status::Failed(e) => {
                    ui.colored_label(egui::Color32::from_rgb(255, 120, 100), format!("Mise à jour impossible : {e}"));
                    ui.horizontal(|ui| {
                        if ui.button("Réessayer").clicked() { action = Action::Check; }
                        if ui.button("Fermer").clicked() { close = true; }
                    });
                }
                Status::Available(m) => {
                    ui.label(format!("La version {} est disponible (vous avez la {}).", m.version, env!("CARGO_PKG_VERSION")));
                    if !m.notes.is_empty() {
                        ui.add_space(4.0);
                        ui.label(&m.notes);
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if installed {
                            if ui.button("Télécharger et installer").clicked() { action = Action::Download(m.clone()); }
                        } else {
                            ui.label("Copie portable : téléchargez la nouvelle version.");
                            if ui.button("Ouvrir la page").clicked() { updater::open_releases_page(); }
                        }
                        if ui.button("Plus tard").clicked() { close = true; }
                    });
                }
                Status::Downloading { done, total } => {
                    ui.label("Téléchargement de la mise à jour…");
                    let f = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
                    ui.add(ProgressBar::new(f).show_percentage());
                }
                Status::Ready { manifest, setup } => {
                    ui.label(format!("La version {} est téléchargée et vérifiée.", manifest.version));
                    if playing {
                        ui.label("Une lecture est en cours : elle ne sera pas interrompue sans votre accord.");
                    }
                    ui.horizontal(|ui| {
                        if playing {
                            if ui.button("Installer à la fermeture").clicked() { action = Action::InstallOnExit(setup.clone()); }
                            if ui.button("Installer maintenant (arrête la lecture)").clicked() { action = Action::InstallNow(setup.clone()); }
                        } else if ui.button("Installer et relancer").clicked() {
                            action = Action::InstallNow(setup.clone());
                        }
                        if ui.button("Plus tard").clicked() { close = true; }
                    });
                }
            }
        });
    if close { *open = false; }
    action
}
