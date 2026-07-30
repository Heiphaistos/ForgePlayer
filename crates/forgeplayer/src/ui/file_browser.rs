use egui::{
    Align, Color32, CornerRadius, FontId, Id, Layout, RichText, ScrollArea,
    Sense, Stroke, Ui, Vec2, Window,
};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use omni_core::SUPPORTED_EXTENSIONS;

const ACCENT:    Color32 = Color32::from_rgb(74, 158, 255);
const BG_ROW:    Color32 = Color32::from_rgb(22, 23, 32);
const BG_HOVER:  Color32 = Color32::from_rgb(35, 37, 52);
const BG_SELECT: Color32 = Color32::from_rgb(28, 50, 90);
const BG_SIDE:   Color32 = Color32::from_rgb(14, 15, 22);
const FG_DIM:    Color32 = Color32::from_gray(100);
const FG_MID:    Color32 = Color32::from_gray(165);
const FG_BRIGHT: Color32 = Color32::from_gray(220);
const FG_FOLDER: Color32 = Color32::from_rgb(220, 185, 80);
const FG_MEDIA:  Color32 = Color32::from_rgb(100, 200, 140);

#[derive(Clone, Debug)]
struct Entry {
    path:     PathBuf,
    name:     String,
    is_dir:   bool,
    is_media: bool,
    size:     Option<u64>,
    modified: Option<SystemTime>,
}

#[derive(Clone, PartialEq)]
enum SortBy { Name, Size, Date }

#[derive(Clone)]
pub struct FileBrowser {
    current_dir: PathBuf,
    entries:     Vec<Entry>,
    selected:    Option<usize>,
    filter:      String,
    sort_by:     SortBy,
    sort_asc:    bool,
    drives:      Vec<String>,
}

impl FileBrowser {
    pub fn new() -> Self {
        let start = dirs::video_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("C:\\"));
        let mut fb = Self {
            current_dir: start,
            entries:     Vec::new(),
            selected:    None,
            filter:      String::new(),
            sort_by:     SortBy::Name,
            sort_asc:    true,
            drives:      detect_drives(),
        };
        fb.refresh();
        fb
    }

    fn refresh(&mut self) {
        self.entries.clear();
        self.selected = None;

        let Ok(read_dir) = std::fs::read_dir(&self.current_dir) else { return };

        let mut dirs_vec:  Vec<Entry> = Vec::new();
        let mut files_vec: Vec<Entry> = Vec::new();

        for entry in read_dir.flatten() {
            let path     = entry.path();
            let name     = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let is_dir   = path.is_dir();
            let is_media = !is_dir && is_media_file(&path);
            let meta     = std::fs::metadata(&path).ok();
            let size     = if is_dir { None } else { meta.as_ref().map(|m| m.len()) };
            let modified = meta.and_then(|m| m.modified().ok());

            let e = Entry { path, name, is_dir, is_media, size, modified };
            if is_dir { dirs_vec.push(e); } else { files_vec.push(e); }
        }

        self.sort_entries(&mut dirs_vec);
        self.sort_entries(&mut files_vec);
        self.entries.extend(dirs_vec);
        self.entries.extend(files_vec);
    }

    fn sort_entries(&self, v: &mut Vec<Entry>) {
        match self.sort_by {
            SortBy::Name => v.sort_by(|a, b| {
                let cmp = a.name.to_lowercase().cmp(&b.name.to_lowercase());
                if self.sort_asc { cmp } else { cmp.reverse() }
            }),
            SortBy::Size => v.sort_by(|a, b| {
                let cmp = a.size.cmp(&b.size);
                if self.sort_asc { cmp } else { cmp.reverse() }
            }),
            SortBy::Date => v.sort_by(|a, b| {
                let cmp = a.modified.cmp(&b.modified);
                if self.sort_asc { cmp } else { cmp.reverse() }
            }),
        }
    }

    fn navigate(&mut self, path: PathBuf) {
        self.current_dir = path;
        self.filter.clear();
        self.refresh();
    }

    #[allow(dead_code)]
    fn selected_path(&self) -> Option<&Path> {
        self.selected.and_then(|i| {
            let e = self.visible_entries().nth(i)?;
            Some(e.path.as_path())
        })
    }

    fn visible_entries(&self) -> impl Iterator<Item = &Entry> {
        let fl = self.filter.to_lowercase();
        self.entries.iter().filter(move |e| {
            fl.is_empty() || e.name.to_lowercase().contains(&fl)
        })
    }
}

pub fn show<F>(ctx: &egui::Context, open: &mut bool, mut on_open: F)
where F: FnMut(String),
{
    let mut browser = ctx.memory_mut(|mem| {
        mem.data
            .get_temp_mut_or_insert_with(Id::new("file_browser_v2"), FileBrowser::new)
            .clone()
    });

    let mut do_open:     Option<String> = None;
    let mut navigated    = false;
    let mut new_dir:     Option<PathBuf> = None;
    let mut should_close = false;
    let mut is_open      = *open;

    Window::new("Ouvrir un fichier")
        .open(&mut is_open)
        .resizable(true)
        .default_size([920.0, 580.0])
        .min_size([640.0, 400.0])
        .show(ctx, |ui| {
            // ── Barre d'adresse + recherche ──────────────────────────────
            address_bar(ui, &browser.current_dir, &mut browser.filter, |p| {
                new_dir = Some(p);
                navigated = true;
            });

            ui.separator();

            // ── Corps : sidebar gauche + liste centrale ──────────────────
            let panel_h = ui.available_height() - 44.0; // réserve pour boutons bas

            ui.horizontal(|ui| {
                // Sidebar
                ui.push_id("sidebar", |ui| {
                    ui.set_min_width(180.0);
                    ui.set_max_width(180.0);
                    ui.set_min_height(panel_h);
                    egui::Frame::new()
                        .fill(BG_SIDE)
                        .inner_margin(egui::Margin::symmetric(4, 4))
                        .show(ui, |ui| {
                            ui.set_min_height(panel_h);
                            sidebar_panel(ui, &browser.current_dir, &browser.drives, |p| {
                                new_dir = Some(p);
                                navigated = true;
                            });
                        });
                });

                ui.separator();

                // Liste fichiers
                ui.vertical(|ui| {
                    // En-têtes de colonnes
                    column_headers(ui, &mut browser.sort_by, &mut browser.sort_asc,
                                   &mut navigated);

                    ScrollArea::vertical()
                        .max_height(panel_h - 28.0)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());

                            let entries: Vec<Entry> = browser.visible_entries().cloned().collect();
                            let mut new_selected = browser.selected;

                            for (vis_idx, entry) in entries.iter().enumerate() {
                                let selected = browser.selected == Some(vis_idx);
                                let resp = file_row(ui, entry, selected);

                                if resp.clicked() {
                                    new_selected = Some(vis_idx);
                                }
                                if resp.double_clicked() {
                                    if entry.is_dir {
                                        new_dir   = Some(entry.path.clone());
                                        navigated = true;
                                    } else if entry.is_media {
                                        do_open = Some(entry.path.to_string_lossy().to_string());
                                    }
                                }
                            }
                            browser.selected = new_selected;
                        });
                });
            });

            ui.separator();

            // ── Barre inférieure ─────────────────────────────────────────
            ui.horizontal(|ui| {
                // Chemin sélectionné
                if let Some(sel) = browser.selected
                    .and_then(|i| browser.visible_entries().nth(i))
                    .filter(|e| !e.is_dir)
                {
                    ui.label(
                        RichText::new(sel.name.as_str())
                            .size(11.5)
                            .color(FG_MID),
                    );
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(6.0);
                    let can_open = browser.selected
                        .and_then(|i| browser.visible_entries().nth(i))
                        .map(|e| e.is_media)
                        .unwrap_or(false);

                    let open_btn = egui::Button::new(
                        RichText::new("  Ouvrir  ").size(13.0).color(Color32::WHITE)
                    )
                    .fill(if can_open { ACCENT } else { Color32::from_gray(50) })
                    .stroke(Stroke::NONE);

                    if ui.add_enabled(can_open, open_btn).clicked() {
                        if let Some(e) = browser.selected
                            .and_then(|i| browser.visible_entries().nth(i))
                        {
                            do_open = Some(e.path.to_string_lossy().to_string());
                        }
                    }

                    ui.add_space(8.0);
                    if ui.button("Annuler").clicked() {
                        should_close = true;
                    }
                });
            });
        });

    if navigated {
        if let Some(p) = new_dir { browser.navigate(p); }
        else                     { browser.refresh(); }
    }

    ctx.memory_mut(|mem| {
        mem.data.insert_temp(Id::new("file_browser_v2"), browser);
    });

    if !is_open || should_close { *open = false; }

    if let Some(path) = do_open {
        *open = false;
        on_open(path);
    }
}

// ─── Barre d'adresse breadcrumb ─────────────────────────────────────────────

fn address_bar<F: FnMut(PathBuf)>(
    ui: &mut Ui,
    current: &Path,
    filter: &mut String,
    mut navigate: F,
) {
    ui.horizontal(|ui| {
        // Breadcrumbs
        let components: Vec<PathBuf> = {
            let mut parts = Vec::new();
            let mut acc = PathBuf::new();
            for c in current.components() {
                acc.push(c);
                parts.push(acc.clone());
            }
            parts
        };

        ui.add_space(4.0);
        for (i, comp_path) in components.iter().enumerate() {
            let label = comp_path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| comp_path.to_string_lossy().to_string());

            if i > 0 {
                ui.label(RichText::new("›").color(FG_DIM).size(13.0));
            }

            let is_last = i == components.len() - 1;
            let color = if is_last { ACCENT } else { FG_MID };
            let btn = ui.add(
                egui::Button::new(RichText::new(&label).size(12.0).color(color))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .min_size(Vec2::new(0.0, 20.0))
            );
            if btn.clicked() && !is_last {
                navigate(comp_path.clone());
            }
        }

        // Séparateur + filtre
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(6.0);
            if !filter.is_empty() {
                if ui.small_button("✕").clicked() { filter.clear(); }
            }
            ui.add(
                egui::TextEdit::singleline(filter)
                    .hint_text("🔍  Filtrer…")
                    .desired_width(160.0)
                    .font(FontId::proportional(11.5))
            );
        });
    });
}

// ─── Sidebar ─────────────────────────────────────────────────────────────────

fn sidebar_panel<F: FnMut(PathBuf)>(
    ui: &mut Ui,
    current: &Path,
    drives: &[String],
    mut navigate: F,
) {
    // Accès rapide
    ui.label(RichText::new("ACCÈS RAPIDE").size(9.5).color(FG_DIM));
    ui.add_space(2.0);

    let quick: &[(&str, fn() -> Option<PathBuf>)] = &[
        ("🖥  Bureau",       || dirs::desktop_dir()),
        ("⬇  Téléchargements", || dirs::download_dir()),
        ("🎬  Vidéos",       || dirs::video_dir()),
        ("🎵  Musique",      || dirs::audio_dir()),
        ("🖼  Images",       || dirs::picture_dir()),
        ("📄  Documents",    || dirs::document_dir()),
    ];

    for (label, dir_fn) in quick {
        if let Some(p) = dir_fn() {
            let active = current == p.as_path();
            side_item(ui, label, active, || navigate(p.clone()));
        }
    }

    ui.add_space(8.0);
    ui.label(RichText::new("LECTEURS").size(9.5).color(FG_DIM));
    ui.add_space(2.0);

    for drive in drives {
        let path = PathBuf::from(drive);
        let active = current.starts_with(&path);
        let label = format!("💾  {drive}");
        side_item(ui, &label, active, || navigate(path.clone()));
    }
}

fn side_item<F: FnOnce()>(ui: &mut Ui, label: &str, active: bool, on_click: F) {
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 24.0),
        Sense::click(),
    );

    if ui.is_rect_visible(rect) {
        let bg = if active         { BG_SELECT }
                 else if resp.hovered() { BG_HOVER }
                 else              { Color32::TRANSPARENT };
        if active {
            ui.painter().rect_filled(rect, CornerRadius::from(3.0_f32), bg);
            ui.painter().line_segment(
                [rect.left_top() + Vec2::new(0.0, 2.0), rect.left_bottom() - Vec2::new(0.0, 2.0)],
                Stroke::new(2.5, ACCENT),
            );
        } else {
            ui.painter().rect_filled(rect, CornerRadius::from(3.0_f32), bg);
        }
        ui.painter().text(
            egui::pos2(rect.left() + 10.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(12.0),
            if active { Color32::WHITE } else { FG_MID },
        );
    }

    if resp.clicked() { on_click(); }
}

// ─── En-têtes de colonnes ────────────────────────────────────────────────────

fn column_headers(ui: &mut Ui, sort_by: &mut SortBy, sort_asc: &mut bool, _navigated: &mut bool) {
    ui.horizontal(|ui| {
        ui.add_space(28.0); // icône

        let hdr = |ui: &mut Ui, label: &str, this: SortBy,
                   sort_by: &mut SortBy, sort_asc: &mut bool, w: f32| {
            let active  = *sort_by == this;
            let arrow   = if active { if *sort_asc { " ▲" } else { " ▼" } } else { "" };
            let color   = if active { ACCENT } else { FG_DIM };
            let btn = ui.add_sized(
                [w, 22.0],
                egui::Button::new(RichText::new(format!("{label}{arrow}")).size(11.0).color(color))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE),
            );
            if btn.clicked() {
                if *sort_by == this { *sort_asc = !*sort_asc; }
                else { *sort_by = this; *sort_asc = true; }
            }
        };

        hdr(ui, "Nom",         SortBy::Name, sort_by, sort_asc, 340.0);
        hdr(ui, "Taille",      SortBy::Size, sort_by, sort_asc, 90.0);
        hdr(ui, "Modifié",     SortBy::Date, sort_by, sort_asc, 140.0);
    });

    ui.painter().line_segment(
        [egui::pos2(ui.min_rect().left(), ui.cursor().top()),
         egui::pos2(ui.min_rect().right(), ui.cursor().top())],
        Stroke::new(1.0, Color32::from_gray(40)),
    );
}

// ─── Ligne de fichier ────────────────────────────────────────────────────────

fn file_row(ui: &mut Ui, entry: &Entry, selected: bool) -> egui::Response {
    let row_h = 26.0;
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), row_h),
        Sense::click(),
    );

    if ui.is_rect_visible(rect) {
        let bg = if selected         { BG_SELECT }
                 else if resp.hovered() { BG_HOVER }
                 else              { BG_ROW };

        ui.painter().rect_filled(rect, CornerRadius::from(2.0_f32), bg);

        // Icône
        let icon = file_icon(entry);
        let icon_color = if entry.is_dir { FG_FOLDER }
                         else if entry.is_media { FG_MEDIA }
                         else { FG_DIM };
        ui.painter().text(
            egui::pos2(rect.left() + 14.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            icon,
            FontId::proportional(14.0),
            icon_color,
        );

        // Nom
        let name_color = if entry.is_dir { FG_FOLDER }
                         else if entry.is_media { FG_BRIGHT }
                         else { FG_MID };
        ui.painter().text(
            egui::pos2(rect.left() + 28.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &entry.name,
            FontId::proportional(12.5),
            name_color,
        );

        // Taille
        if let Some(sz) = entry.size {
            ui.painter().text(
                egui::pos2(rect.left() + 375.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                fmt_size(sz),
                FontId::monospace(11.0),
                FG_DIM,
            );
        }

        // Date
        if let Some(ts) = entry.modified {
            let dt = fmt_date(ts);
            ui.painter().text(
                egui::pos2(rect.left() + 480.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                dt,
                FontId::proportional(11.0),
                FG_DIM,
            );
        }
    }

    resp
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn file_icon(e: &Entry) -> &'static str {
    if e.is_dir { return "📁"; }
    let ext = e.path.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "mp4"|"mkv"|"avi"|"mov"|"wmv"|"flv"|"webm"|"m4v"|"ts"|"m2ts" => "🎬",
        "mp3"|"flac"|"aac"|"ogg"|"wav"|"opus"|"m4a"|"wma"            => "🎵",
        "jpg"|"jpeg"|"png"|"gif"|"bmp"|"webp"|"tiff"|"heic"|"avif"   => "🖼",
        "srt"|"ass"|"ssa"|"vtt"                                        => "💬",
        _                                                               => "📄",
    }
}

fn fmt_size(bytes: u64) -> String {
    const K: u64 = 1024;
    if bytes < K           { format!("{bytes} o") }
    else if bytes < K*K    { format!("{:.1} Ko", bytes as f64 / K as f64) }
    else if bytes < K*K*K  { format!("{:.1} Mo", bytes as f64 / (K*K) as f64) }
    else                   { format!("{:.2} Go", bytes as f64 / (K*K*K) as f64) }
}

fn fmt_date(ts: SystemTime) -> String {
    use std::time::UNIX_EPOCH;
    let secs = ts.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let (y, mo, d, h, mi) = secs_to_ymdhi(secs);
    format!("{d:02}/{mo:02}/{y}  {h:02}:{mi:02}")
}

fn secs_to_ymdhi(secs: u64) -> (u64, u64, u64, u64, u64) {
    let mi  = (secs / 60) % 60;
    let h   = (secs / 3600) % 24;
    let day = secs / 86400;
    // Algorithme calendrier civil (jours depuis 1970-01-01)
    let z   = day + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y   = yoe + era * 400;
    let doy = doe - (365*yoe + yoe/4 - yoe/100);
    let mp  = (5*doy + 2) / 153;
    let d   = doy - (153*mp + 2)/5 + 1;
    let mo  = if mp < 10 { mp + 3 } else { mp - 9 };
    let y   = if mo <= 2 { y + 1 } else { y };
    (y, mo, d, h, mi)
}

fn detect_drives() -> Vec<String> {
    let mut drives = Vec::new();
    #[cfg(windows)]
    for letter in b'A'..=b'Z' {
        let d = format!("{}:\\", letter as char);
        if Path::new(&d).exists() { drives.push(d); }
    }
    #[cfg(not(windows))]
    drives.push("/".to_string());
    drives
}

/// Extensions qu'on sait *ne jamais* pouvoir ouvrir — tout le reste est tenté
/// (FFmpeg sniffe le contenu réel, l'extension n'a pas d'importance pour lui ;
/// filtrer par une liste blanche empêcherait d'ouvrir des fichiers valides avec
/// une extension inhabituelle, renommée ou absente). Si FFmpeg échoue quand
/// même, l'écran d'erreur existant s'affiche normalement.
const NEVER_MEDIA_EXTENSIONS: &[&str] = &[
    "exe", "dll", "sys", "msi", "bat", "cmd", "ps1", "sh",
    "txt", "md", "log", "ini", "cfg", "json", "xml", "yaml", "yml",
    "zip", "rar", "7z", "tar", "gz", "iso",
    "doc", "docx", "xls", "xlsx", "ppt", "pptx", "pdf",
    "ttf", "otf", "woff", "woff2",
    "db", "sqlite", "lnk",
];

fn is_media_file(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    // Toujours ouvrable si dans la liste connue ; sinon ouvrable sauf si
    // explicitement exclu (extension absente = ouvrable, FFmpeg sniffera).
    SUPPORTED_EXTENSIONS.contains(&ext.as_str()) || !NEVER_MEDIA_EXTENSIONS.contains(&ext.as_str())
}
