use egui::{Color32, Context, Key, RichText, TextEdit, Window};

const VALID_SCHEMES: &[&str] = &["http://", "https://", "rtsp://", "rtmp://", "udp://", "file://"];

/// Retourne `Some(url)` quand l'utilisateur valide, `None` si en attente.
pub fn show(ctx: &Context, open: &mut bool, input: &mut String) -> Option<String> {
    let mut result: Option<String> = None;
    // Le champ texte garde le focus clavier en continu (request_focus() ci-dessous,
    // chaque frame) : wants_keyboard_input() reste vrai tant que la fenêtre est
    // ouverte, ce qui bloque Échap au niveau global (handle_keyboard dans app.rs).
    // On lit donc Échap directement ici, en amont de ce filtrage.
    let mut should_close = ctx.input(|i| i.key_pressed(Key::Escape));

    Window::new("🔗  Ouvrir une URL")
        .open(open)
        .resizable(false)
        .collapsible(false)
        .default_size([440.0, 100.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("URL du flux ou fichier réseau :").size(12.0));
            ui.add_space(4.0);

            let resp = ui.add(
                TextEdit::singleline(input)
                    .hint_text("https://… ou rtsp://… ou file://…")
                    .desired_width(f32::INFINITY),
            );
            resp.request_focus();

            let valid = is_valid_url(input);
            let err_color = Color32::from_rgb(255, 100, 80);

            if !input.is_empty() && !valid {
                ui.label(RichText::new("⚠ Schéma non supporté (http/https/rtsp/rtmp/udp/file)").color(err_color).small());
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ok_clicked = ui.add_enabled(valid, egui::Button::new("▶ Ouvrir")).clicked();
                let enter = resp.lost_focus() && ctx.input(|i| i.key_pressed(Key::Enter));

                if (ok_clicked || enter) && valid {
                    result = Some(input.trim().to_string());
                    should_close = true;
                }

                if ui.button("Annuler").clicked() {
                    should_close = true;
                    input.clear();
                }
            });
        });

    if should_close { *open = false; }
    result
}

fn is_valid_url(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() { return false; }
    VALID_SCHEMES.iter().any(|scheme| s.starts_with(scheme))
}
