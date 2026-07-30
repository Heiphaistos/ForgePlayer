use eframe::{CreationContext, Frame};
use egui::{CentralPanel, Context, Id, Key, Order};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use parking_lot::Mutex;

use crate::config::AppConfig;
use crate::player::{Player, PlayerState};
use crate::services::ServicesClient;
use crate::ui::{controls, file_browser, info_overlay, player_view, playlist, settings, url_dialog};
use crate::ui::image_viewer::ImageViewer;
use crate::video_callback::SharedFrame;
use omni_audio::AudioEngine;
use omni_renderer::VideoRenderer;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(74, 158, 255);

struct Osd { text: String, expires_at: f64 }

pub struct OmniApp {
    player:            Player,
    audio:             Option<AudioEngine>,
    config:            AppConfig,
    show_settings:     bool,
    show_playlist:     bool,
    show_file_browser: bool,
    show_url_dialog:   bool,
    show_info:         bool,
    url_input:         String,
    is_fullscreen:     bool,
    playlist_items:    Vec<String>,
    playlist_idx:      Option<usize>,
    seek_request:      Option<f64>,
    video_frame:       SharedFrame,
    /// Vrai si le frame actuellement affiché est HDR 10-bit — état persistant
    /// (pas seulement "nouveau frame ce tick") pour que le chemin de rendu
    /// (SDR direct vs HDR deux passes) reste cohérent entre deux frames
    /// décodées, y compris pendant la pause.
    video_is_hdr:      Arc<AtomicBool>,
    osd:               Option<Osd>,
    #[allow(dead_code)] services: Option<ServicesClient>,
    last_mouse_move:   f64,
    image_viewer:      ImageViewer,
    image_texture:     Option<egui::TextureHandle>,
    image_path_loaded: String,
    pending_video_frame: Option<omni_core::decoder::DecodedVideoFrame>,
    video_color_space: u32,   // 0=BT601, 1=BT709, 2=BT2020
    last_title:        String, // pour détecter les changements de fichier
    paused_preview:    bool,   // seek en pause : afficher la frame preview à venir
    dbg_start:         Option<std::time::Instant>,
    dbg_last_log:      f64,
}

impl OmniApp {
    pub fn new(cc: &CreationContext, config: AppConfig, initial_file: Option<String>) -> Self {
        Self::apply_theme(&cc.egui_ctx);

        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            let mut resources = rs.renderer.write();
            match VideoRenderer::new(&rs.device, rs.target_format) {
                Ok(r)  => { resources.callback_resources.insert(r); }
                Err(e) => log::error!("VideoRenderer init: {e}"),
            }
            // Second pass (chemin HDR) : tone mapping PQ→SDR. Toujours créé —
            // coût quasi nul tant qu'aucun contenu HDR n'est lu (bind group
            // vide, render() ne dessine rien).
            resources.callback_resources.insert(
                omni_renderer::HdrTonemapper::new(&rs.device, rs.target_format)
            );
            resources.callback_resources.insert(crate::video_callback::HdrOffscreen::default());
        }

        let audio = AudioEngine::new()
            .map_err(|e| log::error!("AudioEngine: {e}")).ok();

        let svc = ServicesClient::new(config.subtitle_service_port, config.media_indexer_port);
        let (services, initial_osd) = if svc.is_subtitle_service_up() {
            (Some(svc), None)
        } else {
            log::warn!(
                "Services sous-titres/indexeur non disponibles (ports {}/{}). \
                 Lancez launch.bat pour les activer.",
                config.subtitle_service_port,
                config.media_indexer_port,
            );
            let msg = "Services sous-titres non disponibles — lancez launch.bat pour les activer";
            (None, Some(Osd { text: msg.to_string(), expires_at: 0.0 }))
        };

        let mut app = Self {
            player: Player::new(), audio, config,
            show_settings: false, show_playlist: false,
            show_file_browser: false, show_url_dialog: false,
            show_info: false,
            url_input: String::new(), is_fullscreen: false,
            playlist_items: Vec::new(), playlist_idx: None, seek_request: None,
            video_frame: Arc::new(Mutex::new(None)),
            video_is_hdr: Arc::new(AtomicBool::new(false)),
            osd: initial_osd, services,
            last_mouse_move: 0.0,
            image_viewer: ImageViewer::default(),
            image_texture: None,
            image_path_loaded: String::new(),
            pending_video_frame: None,
            video_color_space: 1,
            last_title: String::new(),
            paused_preview: false,
            dbg_start: None,
            dbg_last_log: 0.0,
        };

        // Restaure volume et vitesse de la session précédente
        app.player.volume = app.config.volume.clamp(0.0, 2.0);
        app.player.set_speed(app.config.playback_speed.clamp(0.25, 4.0));

        // Fichier passé en ligne de commande (« Ouvrir avec » Windows)
        if let Some(path) = initial_file {
            app.playlist_items.push(path.clone());
            app.playlist_idx = Some(0);
            app.open_file(path);
        }

        app
    }

    fn apply_theme(ctx: &Context) {
        let mut v = egui::Visuals::dark();
        v.window_corner_radius      = egui::CornerRadius::from(10.0_f32);
        v.panel_fill                = egui::Color32::from_rgb(10, 10, 16);
        v.window_fill               = egui::Color32::from_rgb(16, 16, 24);
        v.window_shadow             = egui::Shadow {
            offset: [0, 4].into(),
            blur: 20,
            spread: 0,
            color: egui::Color32::from_black_alpha(120),
        };
        v.widgets.inactive.bg_fill  = egui::Color32::from_rgb(26, 26, 38);
        v.widgets.inactive.corner_radius = egui::CornerRadius::from(5.0_f32);
        v.widgets.hovered.bg_fill   = egui::Color32::from_rgb(36, 46, 72);
        v.widgets.hovered.corner_radius  = egui::CornerRadius::from(5.0_f32);
        v.widgets.active.bg_fill    = egui::Color32::from_rgb(55, 100, 200);
        v.widgets.active.corner_radius   = egui::CornerRadius::from(5.0_f32);
        v.selection.bg_fill         = egui::Color32::from_rgb(40, 80, 160);
        v.hyperlink_color           = ACCENT;
        v.override_text_color       = Some(egui::Color32::from_gray(225));
        ctx.set_visuals(v);
    }

    fn controls_visible(&self, now: f64) -> bool {
        !matches!(self.player.state, PlayerState::Playing)
            || (now - self.last_mouse_move) < 3.0
    }

    fn open_file(&mut self, path: String) {
        log::info!("ouverture: {path}");
        *self.video_frame.lock() = None;
        self.video_is_hdr.store(false, Ordering::Relaxed);
        self.pending_video_frame = None;
        self.config.add_recent(&path);
        // Reset image viewer pour nouvelle image
        self.image_viewer.reset();
        self.image_texture = None;
        self.image_path_loaded.clear();
        if let Err(e) = self.player.open(&path) {
            log::error!("player.open: {e}");
            self.player.state = PlayerState::Error(e.to_string());
        }
        // APRÈS open() — open() réinitialise subtitle_track, l'inverse le perdait
        self.try_load_adjacent_subtitle(&path);
    }

    fn try_load_adjacent_subtitle(&mut self, media_path: &str) {
        if omni_core::is_image_path(media_path) { return; }
        let base = std::path::Path::new(media_path).with_extension("");
        for ext in &["srt", "ass", "ssa"] {
            let sub = base.with_extension(ext);
            if sub.exists() {
                if self.player.load_subtitle(&sub.to_string_lossy()).is_ok() {
                    let name = sub.file_name().unwrap_or_default().to_string_lossy().to_string();
                    log::info!("sous-titre adjacent chargé: {name}");
                    self.set_osd(format!("Sous-titre : {name}"));
                }
                break;
            }
        }
    }

    fn set_osd(&mut self, text: impl Into<String>) {
        self.osd = Some(Osd { text: text.into(), expires_at: 0.0 });
    }

    fn osd_text(&self, now: f64) -> Option<&str> {
        self.osd.as_ref()
            .filter(|o| o.expires_at == 0.0 || o.expires_at > now)
            .map(|o| o.text.as_str())
    }

    fn process_seek(&mut self) {
        if let Some(pos) = self.seek_request.take() {
            self.player.seek(pos);
            self.pending_video_frame = None;
        }
    }

    /// Horloge maître = audio réellement joué. Sans ça, l'horloge suit la position
    /// de DÉCODAGE (en avance de ~14 s de buffers) et la vidéo court devant l'audio.
    fn sync_clock_to_audio(&mut self) {
        self.player.clock_audio_master = false;
        if self.player.is_image_mode() { return; }
        let has_audio = self.player.media_info.as_ref()
            .map(|i| !i.audio.is_empty()).unwrap_or(false);
        if !has_audio { return; }
        // Vitesse ≠ 1 : l'audio joue toujours à 1× (pas de time-stretch) — on
        // laisse l'horloge murale piloter, l'audio sera désynchronisé (limitation).
        if (self.player.speed() - 1.0).abs() > 0.01 { return; }
        let Some(audio) = &self.audio else { return };
        let Some(pos) = audio.playback_position() else {
            // Pas encore de données post-flush : on fige l'horloge sur la position
            // connue plutôt que de laisser les événements de décodage la pousser.
            self.player.clock_audio_master = true;
            return;
        };
        self.player.clock_audio_master = true;
        if matches!(self.player.state, PlayerState::Playing | PlayerState::Buffering(_)) {
            self.player.position = pos;
            self.player.clock.update(pos);
        }
    }

    /// Quand rien ne pilote l'horloge en continu (pas d'audio, pas de piste
    /// audio, ou vitesse ≠ 1×), l'horloge tourne en roue libre sur le temps réel
    /// (amorcée une fois au démarrage/seek, puis `elapsed*speed`). `position`
    /// doit refléter CETTE horloge, pas le PTS brut de la dernière frame
    /// décodée (sinon la position affichée == vitesse de décodage, pas le
    /// temps réel — c'est ce qui causait la dérive en VM sans rendu GPU).
    fn sync_position_from_clock(&mut self) {
        if self.player.clock_audio_master { return; }
        if matches!(self.player.state, PlayerState::Playing | PlayerState::Buffering(_)) {
            self.player.position = self.player.clock.position_secs();
        }
    }

    /// Purge demandée par le player (seek / nouveau fichier) : audio buffurisé
    /// (jusqu'à 8 s de ring) + frame vidéo en attente.
    fn process_flush(&mut self) {
        if !self.player.audio_flush_needed { return; }
        self.player.audio_flush_needed = false;
        if let Some(a) = &self.audio { a.flush(); }
        self.pending_video_frame = None;
        // Seek pendant la pause : le demuxer va envoyer une frame de preview
        if self.player.state == PlayerState::Paused {
            self.paused_preview = true;
        }
    }

    fn pump_audio(&mut self) {
        let Some(audio) = &self.audio else {
            // Pas de périphérique de sortie (device absent/erreur d'ouverture, ex.
            // AudioEngine::new() a échoué). Le pipeline pousse quand même des frames
            // audio dans sa queue (audio_tx, capacité 512) et le demuxer ATTEND que
            // cette queue se vide avant de continuer à lire le fichier (régulation
            // anti-surproduction). Sans drainage ici, la queue se remplit une fois
            // pour toutes et ne se vide plus jamais → demuxer bloqué en permanence →
            // vidéo figée + seek qui "ne fait plus rien" (repositionne en interne
            // mais ne peut plus produire de nouvelles frames). On drape donc
            // toujours la queue, même sans rien en faire.
            while self.player.try_recv_audio_frame().is_some() {}
            return;
        };
        audio.set_paused(self.player.state == PlayerState::Paused);
        audio.set_volume(self.player.effective_volume());

        // Régulation : vise ~4 s de ring (capacité 8 s), max 32 frames par repaint.
        // Le demuxer se met en attente quand audio_rx (512) est plein — chaîne de
        // backpressure complète, plus aucun drop en régime établi.
        let mut pushed = 0;
        while audio.buffered_secs() < 4.0 && pushed < 32 {
            let Some(frame) = self.player.try_recv_audio_frame() else { break };
            audio.push_frame(frame);
            pushed += 1;
        }
    }

    fn pump_video(&mut self) {
        if self.player.is_image_mode() { return; }

        // Seek effectué en pause : affiche la frame de preview dès son arrivée
        // (la queue a été purgée au seek — la seule frame qui arrive est la preview)
        if self.player.state == PlayerState::Paused {
            if self.paused_preview {
                if let Some(frame) = self.player.try_recv_video_frame() {
                    self.set_video_frame(frame);
                    self.paused_preview = false;
                }
            }
            return;
        }
        self.paused_preview = false;

        // Loading inclus : draine la queue vidéo dès le départ pour ne pas bloquer
        // le demuxer (régulation par queues pleines) avant le passage en Playing
        if !matches!(self.player.state,
            PlayerState::Playing | PlayerState::Buffering(_) | PlayerState::Loading) { return; }

        let clock = self.player.clock.position_secs();

        // Si on a un frame en attente, l'afficher dès que son PTS est atteint
        if let Some(ref pf) = self.pending_video_frame {
            let diff = pf.pts_secs - clock;
            // Affiche si PTS atteint (diff <= 20ms) OU si le frame attend depuis trop longtemps
            // (clock gelé → afficher quand même pour éviter un freeze total)
            if diff <= 0.020 || diff > 2.0 {
                let frame = self.pending_video_frame.take().unwrap();
                self.set_video_frame(frame);
            }
            return;
        }

        // Dépiler des frames de la queue
        loop {
            let Some(frame) = self.player.try_recv_video_frame() else { break };

            if frame.pts_secs < clock - 0.100 {
                // Frame en retard de plus de 100 ms → dropper, continuer
                continue;
            }
            if frame.pts_secs <= clock + 0.020 {
                // Frame due maintenant → afficher
                self.set_video_frame(frame);
            } else {
                // Frame trop en avance → mettre en attente, stopper
                self.pending_video_frame = Some(frame);
            }
            break;
        }
    }

    /// Publie un frame décodé vers le callback de rendu, et met à jour l'état
    /// HDR persistant utilisé pour choisir le chemin de rendu (SDR direct vs
    /// HDR deux passes) même sur les repaints sans nouvelle frame.
    fn set_video_frame(&mut self, frame: omni_core::decoder::DecodedVideoFrame) {
        self.video_is_hdr.store(frame.format.is_hdr10bit(), Ordering::Relaxed);
        *self.video_frame.lock() = Some(frame);
    }

    fn ensure_image_texture(&mut self, ctx: &Context) {
        if let Some(img) = &self.player.image_frame {
            if self.image_path_loaded != img.path {
                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                    [img.width as usize, img.height as usize],
                    &img.pixels,
                );
                self.image_texture = Some(ctx.load_texture(
                    "image_viewer",
                    color_img,
                    egui::TextureOptions::LINEAR,
                ));
                self.image_path_loaded = img.path.clone();
            }
        }
    }

    fn handle_keyboard(&mut self, ctx: &Context, now: f64) {
        if ctx.wants_keyboard_input() { return; }

        let keys = ctx.input(|i| {
            let shift = i.modifiers.shift;
            let alt   = i.modifiers.alt;
            let ctrl  = i.modifiers.ctrl;
            (
                i.key_pressed(Key::Space),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::F),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::S),
                i.key_pressed(Key::A),
                i.key_pressed(Key::M),
                i.key_pressed(Key::N),
                !ctrl && i.key_pressed(Key::P),
                i.key_pressed(Key::I),
                ctrl && i.key_pressed(Key::O),
                ctrl && i.key_pressed(Key::L),
                ctrl && i.key_pressed(Key::P),
                ctrl && i.key_pressed(Key::Q),
                i.key_pressed(Key::W),
                !ctrl && i.key_pressed(Key::L),
                i.key_pressed(Key::OpenBracket),
                i.key_pressed(Key::CloseBracket),
                shift,
                alt,
            )
        });
        let (k_space, k_left, k_right, k_up, k_down,
             k_f, k_esc, k_s, k_a, k_m, k_n, k_p, k_i,
             k_ctrl_o, k_ctrl_l, k_ctrl_p, k_ctrl_q,
             k_w, k_l, k_open_br, k_close_br,
             shift, alt) = keys;

        if k_space  { self.player.play_pause(); }

        // Seek avec modificateurs: Alt=±1s, Shift=±60s, normal=±10s
        if k_left {
            let delta = if alt { -1.0 } else if shift { -60.0 } else { -10.0 };
            self.player.seek_relative(delta);
            self.set_osd(format!("{delta:+.0} s"));
            self.last_mouse_move = now;
        }
        if k_right {
            let delta = if alt { 1.0 } else if shift { 60.0 } else { 10.0 };
            self.player.seek_relative(delta);
            self.set_osd(format!("{delta:+.0} s"));
            self.last_mouse_move = now;
        }

        if k_up {
            let v = (self.player.volume + 0.1).min(2.0);
            self.player.set_volume(v);
            self.set_osd(format!("Volume {:.0}%", v * 100.0));
        }
        if k_down {
            let v = (self.player.volume - 0.1).max(0.0);
            self.player.set_volume(v);
            self.set_osd(format!("Volume {:.0}%", v * 100.0));
        }
        if k_m {
            self.player.toggle_mute();
            self.set_osd(if self.player.muted { "Muet" } else { "Son actif" });
        }
        if k_s      { self.player.next_subtitle_track(); }
        if k_a      { self.player.next_audio_track(); }
        if k_n      { self.playlist_next(); }
        if k_p      { self.playlist_prev(); }
        if k_i      { self.show_info = !self.show_info; }
        if k_ctrl_o { self.show_file_browser = true; }
        if k_ctrl_l { self.show_url_dialog   = true; }
        if k_ctrl_p { self.show_playlist     = !self.show_playlist; }
        if k_ctrl_q { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }

        if k_w {
            self.config.aspect_mode = self.config.aspect_mode.next();
            self.set_osd(format!("Format : {}", self.config.aspect_mode.label()));
        }

        if k_l {
            self.config.loop_mode = self.config.loop_mode.next();
            self.set_osd(format!("Répétition : {}", self.config.loop_mode.label()));
        }

        // Vitesse [ = ralentir, ] = accélérer
        const SPEEDS: &[f32] = &[0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 4.0];
        if k_open_br {
            let cur = self.player.speed();
            if let Some(&s) = SPEEDS.iter().rev().find(|&&s| s < cur - 0.01) {
                self.apply_speed(s);
            }
        }
        if k_close_br {
            let cur = self.player.speed();
            if let Some(&s) = SPEEDS.iter().find(|&&s| s > cur + 0.01) {
                self.apply_speed(s);
            }
        }

        if k_f {
            self.is_fullscreen = !self.is_fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
        }
        if k_esc {
            if self.is_fullscreen {
                self.is_fullscreen = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            } else if self.show_file_browser {
                self.show_file_browser = false;
            } else if self.show_settings {
                self.show_settings = false;
            }
        }
    }

    fn apply_speed(&mut self, speed: f32) {
        self.player.set_speed(speed);
        self.config.playback_speed = speed;
        if (speed - 1.0).abs() < 0.01 {
            self.set_osd("Vitesse : 1×".to_string());
        } else {
            self.set_osd(format!("Vitesse : {speed}×"));
        }
    }

    fn handle_drop(&mut self, ctx: &Context) {
        let dropped: Vec<String> = ctx.input(|i|
            i.raw.dropped_files.iter()
                .filter_map(|f| f.path.as_ref().map(|p| p.to_string_lossy().to_string()))
                .collect()
        );
        for path in dropped {
            let lower = path.to_lowercase();
            let is_sub = lower.ends_with(".srt") || lower.ends_with(".ass")
                      || lower.ends_with(".ssa") || lower.ends_with(".vtt");
            if is_sub {
                if self.player.load_subtitle(&path).is_ok() { self.set_osd("Sous-titre chargé"); }
            } else {
                if !self.playlist_items.contains(&path) { self.playlist_items.push(path.clone()); }
                if self.playlist_idx.is_none() {
                    let idx = self.playlist_items.len() - 1;
                    self.playlist_idx = Some(idx);
                    self.open_file(path);
                }
            }
        }
    }

    fn detect_color_space(info: &omni_core::probe::MediaInfo) -> u32 {
        if let Some(v) = &info.video {
            let cs = v.color_space.to_lowercase();
            if cs.contains("bt470") || cs.contains("smpte170") || cs.contains("bt601") {
                return 0;
            }
            if cs.contains("bt2020") || v.hdr {
                return 2;
            }
            if cs.contains("bt709") {
                return 1;
            }
            // Heuristique résolution: SD → BT.601, 4K HDR → BT.2020
            let px = v.width * v.height;
            if px <= 640 * 480 { return 0; }
            if px >= 3840 * 2160 && v.hdr { return 2; }
        }
        1 // BT.709 par défaut
    }

    fn update_window_title(&mut self, ctx: &egui::Context) {
        let title = self.player.display_title()
            .map(|t| format!("OmniPlayer — {t}"))
            .unwrap_or_else(|| "OmniPlayer".to_string());
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    fn playlist_next(&mut self) {
        if let Some(idx) = self.playlist_idx {
            let next = idx + 1;
            if next < self.playlist_items.len() {
                let path = self.playlist_items[next].clone();
                self.playlist_idx = Some(next);
                self.open_file(path);
            }
        }
    }

    fn playlist_prev(&mut self) {
        if let Some(idx) = self.playlist_idx {
            if idx > 0 {
                let prev = idx - 1;
                let path = self.playlist_items[prev].clone();
                self.playlist_idx = Some(prev);
                self.open_file(path);
            }
        }
    }

    fn save_playlist(&mut self) {
        if self.playlist_items.is_empty() { return; }
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Playlist M3U8", &["m3u8"])
            .set_file_name("playlist.m3u8")
            .save_file()
        {
            match crate::playlist_io::save_m3u(&self.playlist_items, &path) {
                Ok(()) => self.set_osd(format!("Playlist enregistrée : {}",
                    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())),
                Err(e) => {
                    log::error!("save_playlist: {e:#}");
                    self.set_osd("Échec de l'enregistrement de la playlist");
                }
            }
        }
    }

    fn load_playlist(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Playlist", &["m3u", "m3u8"])
            .pick_file()
        {
            match crate::playlist_io::load_m3u(&path) {
                Ok(items) if items.is_empty() => {
                    self.set_osd("Playlist vide ou fichiers introuvables");
                }
                Ok(items) => {
                    let count = items.len();
                    self.playlist_items = items;
                    self.playlist_idx = None;
                    self.set_osd(format!("Playlist chargée : {count} élément(s)"));
                }
                Err(e) => {
                    log::error!("load_playlist: {e:#}");
                    self.set_osd("Échec du chargement de la playlist");
                }
            }
        }
    }
}

impl eframe::App for OmniApp {
    fn update(&mut self, ctx: &Context, _frame: &mut Frame) {
        let now = ctx.input(|i| i.time);

        // Tracking mouvement souris pour auto-hide
        if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_pressed()) {
            self.last_mouse_move = now;
        }

        // OSD expiration
        if let Some(osd) = &mut self.osd {
            if osd.expires_at == 0.0 { osd.expires_at = now + 2.5; }
        }

        // Pipeline
        self.sync_clock_to_audio();
        self.player.poll_events();
        self.sync_position_from_clock();
        self.process_seek();
        self.process_flush();
        self.pump_audio();
        self.pump_video();

        // Sonde diagnostic (silencieuse par défaut — RUST_LOG=debug pour activer) :
        // wall-clock réel vs position lue, pour détecter une dérive audio/vidéo.
        if matches!(self.player.state, PlayerState::Playing) {
            if self.dbg_start.is_none() { self.dbg_start = Some(std::time::Instant::now()); }
            let wall = self.dbg_start.unwrap().elapsed().as_secs_f64();
            if wall - self.dbg_last_log >= 3.0 {
                self.dbg_last_log = wall;
                let buffered = self.audio.as_ref().map(|a| a.buffered_secs()).unwrap_or(-1.0);
                let raw_pos = self.audio.as_ref().and_then(|a| a.playback_position());
                log::debug!("DBGPROBE wall={wall:.2} pos={:.2} clock={:.2} audio_master={} buffered={buffered:.2} raw_audio_pos={raw_pos:?}",
                    self.player.position, self.player.clock.position_secs(), self.player.clock_audio_master);
            }
        } else {
            self.dbg_start = None;
            self.dbg_last_log = 0.0;
        }
        self.ensure_image_texture(ctx);

        // Détection espace colorimétrique lors du chargement des métadonnées
        if let Some(info) = &self.player.media_info {
            let cs = Self::detect_color_space(info);
            self.video_color_space = cs;
        }

        // Titre fenêtre dynamique
        self.update_window_title(ctx);

        if self.player.state == PlayerState::EndOfFile {
            self.pending_video_frame = None;
            match &self.config.loop_mode.clone() {
                crate::config::LoopMode::One => {
                    // Le pipeline est terminé après EOF : replay relance un pipeline
                    // complet (seek seul serait envoyé à un thread mort)
                    self.player.replay();
                }
                crate::config::LoopMode::All => {
                    let next = self.playlist_idx.map(|i| i + 1).unwrap_or(0);
                    let path = if next < self.playlist_items.len() {
                        self.playlist_idx = Some(next);
                        Some(self.playlist_items[next].clone())
                    } else if !self.playlist_items.is_empty() {
                        self.playlist_idx = Some(0);
                        Some(self.playlist_items[0].clone())
                    } else { None };
                    if let Some(p) = path { self.open_file(p); }
                }
                crate::config::LoopMode::Off => {
                    // Essaie de passer au suivant ; si aucun, repasse en Paused
                    // pour que les contrôles restent utilisables
                    let had_next = if let Some(idx) = self.playlist_idx {
                        let next = idx + 1;
                        if next < self.playlist_items.len() {
                            let path = self.playlist_items[next].clone();
                            self.playlist_idx = Some(next);
                            self.open_file(path);
                            true
                        } else { false }
                    } else { false };

                    if !had_next {
                        // Fin de playlist : rester sur la dernière frame en EndOfFile.
                        // Espace ou seek relancent le média (Player::replay).
                        self.player.clock.pause();
                    }
                }
            }
        }

        // Entrées clavier
        self.handle_keyboard(ctx, now);
        self.handle_drop(ctx);

        // Drag-over highlight
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("drop_hint")))
                .rect_stroke(
                    ctx.screen_rect(), 0.0,
                    egui::Stroke::new(3.0, ACCENT),
                    egui::StrokeKind::Middle,
                );
        }

        let controls_vis = self.controls_visible(now);

        // ── Menu bar (masqué en plein écran quand contrôles cachés) ────────
        let show_menu = !self.is_fullscreen || controls_vis;
        if show_menu {
            egui::TopBottomPanel::top("top_bar")
                .frame(egui::Frame::new()
                    .fill(egui::Color32::from_rgb(12, 12, 18))
                    .inner_margin(egui::Margin::symmetric(4, 2)))
                .show(ctx, |ui| {
                    self.draw_menu_bar(ui, now);
                });
        }

        // ── Playlist ──────────────────────────────────────────────────────
        if self.show_playlist {
            let mut play_path: Option<String> = None;
            let mut panel_action = playlist::PlaylistAction::None;
            egui::SidePanel::right("playlist_panel")
                .resizable(true).default_width(270.0)
                .frame(egui::Frame::new()
                    .fill(egui::Color32::from_rgb(12, 12, 20))
                    .inner_margin(egui::Margin::same(8)))
                .show(ctx, |ui| {
                    panel_action = playlist::show(ui, &mut self.playlist_items, &mut self.playlist_idx,
                        |path| play_path = Some(path));
                });
            if let Some(path) = play_path { self.open_file(path); }
            match panel_action {
                playlist::PlaylistAction::Add  => { self.show_file_browser = true; }
                playlist::PlaylistAction::Save => self.save_playlist(),
                playlist::PlaylistAction::Load => self.load_playlist(),
                playlist::PlaylistAction::None => {}
            }
        }

        // ── Zone centrale (vidéo) ─────────────────────────────────────────
        let mut toggle_fs = false;
        CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                let osd = self.osd_text(now).map(|s| s.to_string());
                toggle_fs = player_view::show(
                    ui, &self.player,
                    Arc::clone(&self.video_frame),
                    osd.as_deref(),
                    self.image_texture.as_ref(),
                    &mut self.image_viewer,
                    &self.config.aspect_mode,
                    self.video_color_space,
                    self.video_is_hdr.load(Ordering::Relaxed),
                    self.config.tonemap_mode,
                    self.config.max_luminance,
                );
            });
        if toggle_fs {
            self.is_fullscreen = !self.is_fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
        }

        // ── Contrôles overlay (auto-hide) ─────────────────────────────────
        if controls_vis {
            let screen = ctx.screen_rect();
            let mut speed_out: Option<f32> = None;
            egui::Area::new(Id::new("controls_overlay"))
                .fixed_pos(egui::pos2(screen.left(), screen.bottom() - 92.0))
                .order(Order::Foreground)
                .show(ctx, |ui| {
                    ui.set_width(screen.width());
                    controls::show(
                        ui, &mut self.player, &mut self.seek_request,
                        &mut self.config.loop_mode,
                        &mut self.config.aspect_mode,
                        &mut speed_out,
                        self.audio.is_some(),
                    );
                });
            if let Some(s) = speed_out {
                self.apply_speed(s);
            }
        }

        // ── Info overlay ──────────────────────────────────────────────────
        if self.show_info {
            if let Some(info) = &self.player.media_info.clone() {
                let is_hdr = info.video.as_ref().map(|v| v.hdr).unwrap_or(false);
                let stats = info_overlay::RuntimeStats {
                    buffered_secs: self.audio.as_ref().map(|a| a.buffered_secs()).unwrap_or(0.0),
                    clock_secs:    self.player.clock.position_secs(),
                    speed:         self.player.speed(),
                    aspect_label:  self.config.aspect_mode.label(),
                    loop_label:    self.config.loop_mode.label(),
                    color_space:   self.video_color_space,
                };
                info_overlay::show(ctx, info, is_hdr, Some(&stats));
            }
        }

        // ── Modaux ────────────────────────────────────────────────────────
        if self.show_file_browser {
            let mut fb_open: Option<String> = None;
            file_browser::show(ctx, &mut self.show_file_browser, |path| {
                fb_open = Some(path);
            });
            if let Some(path) = fb_open {
                if !self.playlist_items.contains(&path) { self.playlist_items.push(path.clone()); }
                if let Some(idx) = self.playlist_items.iter().position(|x| x == &path) {
                    self.playlist_idx = Some(idx);
                }
                self.open_file(path);
            }
        }

        if self.show_url_dialog {
            if let Some(url) = url_dialog::show(ctx, &mut self.show_url_dialog, &mut self.url_input) {
                self.open_file(url);
                self.url_input.clear();
            }
        }

        if self.show_settings {
            settings::show(ctx, &mut self.show_settings, &mut self.config);
        }

        // Repaint cadencé à ~60 fps max quand actif (évite over-polling)
        if self.player.is_active() {
            ctx.request_repaint_after(std::time::Duration::from_millis(14));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.player.stop();
        // Persiste volume et vitesse de lecture pour la prochaine session
        self.config.volume = self.player.volume;
        self.config.playback_speed = self.player.speed();
        self.config.save();
    }
}

impl OmniApp {
    fn draw_menu_bar(&mut self, ui: &mut egui::Ui, _now: f64) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("Fichier", |ui| {
                if ui.button("📂 Ouvrir…  Ctrl+O").clicked() {
                    self.show_file_browser = true; ui.close_menu();
                }
                if ui.button("🔗 Ouvrir URL…  Ctrl+L").clicked() {
                    self.show_url_dialog = true; ui.close_menu();
                }
                ui.separator();
                ui.menu_button("🕐 Récents", |ui| {
                    let recents = self.config.recent_files.clone();
                    for f in &recents {
                        let name = std::path::Path::new(f)
                            .file_name().map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| f.clone());
                        if ui.button(name).on_hover_text(f).clicked() {
                            let p = f.clone(); self.open_file(p); ui.close_menu();
                        }
                    }
                    if recents.is_empty() {
                        ui.label(egui::RichText::new("(vide)").color(egui::Color32::from_gray(110)));
                    }
                });
                ui.separator();
                if ui.button("💬 Charger sous-titre…").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("Sous-titres", &["srt", "ass", "ssa", "vtt"])
                        .pick_file()
                    {
                        let s = p.to_string_lossy().to_string();
                        if self.player.load_subtitle(&s).is_ok() { self.set_osd("Sous-titre chargé"); }
                    }
                    ui.close_menu();
                }
                if ui.button("✕ Effacer sous-titre").clicked() {
                    self.player.clear_subtitle(); ui.close_menu();
                }
                ui.separator();
                if ui.button("💾 Enregistrer la playlist…").clicked() {
                    self.save_playlist(); ui.close_menu();
                }
                if ui.button("📂 Charger une playlist…").clicked() {
                    self.load_playlist(); ui.close_menu();
                }
                ui.separator();
                if ui.button("⏻ Quitter  Ctrl+Q").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            ui.menu_button("Vue", |ui| {
                ui.checkbox(&mut self.show_playlist, "Playlist  Ctrl+P");
                if ui.checkbox(&mut self.show_info, "Infos média  I").changed() {}
                ui.separator();
                let fs_label = if self.is_fullscreen { "🗗 Quitter plein écran  F" } else { "⛶ Plein écran  F" };
                if ui.button(fs_label).clicked() {
                    self.is_fullscreen = !self.is_fullscreen;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
                    ui.close_menu();
                }
            });

            ui.menu_button("Lecture", |ui| {
                if ui.button("▶/⏸  Espace").clicked()          { self.player.play_pause(); ui.close_menu(); }
                if ui.button("⏮  Ch. précédent").clicked()      { self.player.chapter_prev(); ui.close_menu(); }
                if ui.button("⏭  Ch. suivant").clicked()        { self.player.chapter_next(); ui.close_menu(); }
                ui.separator();
                if ui.button("🎵  Piste audio  A").clicked()    { self.player.next_audio_track(); ui.close_menu(); }
                if ui.button("💬  Sous-titres  S").clicked()    { self.player.next_subtitle_track(); ui.close_menu(); }
                if ui.button("🔇  Muet  M").clicked()           { self.player.toggle_mute(); ui.close_menu(); }
            });

            ui.menu_button("Outils", |ui| {
                if ui.button("⚙ Paramètres").clicked() {
                    self.show_settings = true; ui.close_menu();
                }
            });

            // Droite : résolution + codec + HDR + titre
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new("OmniPlayer").color(ACCENT).strong().size(13.0));
                if let Some(info) = &self.player.media_info {
                    if let Some(v) = &info.video {
                        ui.separator();
                        let res = omni_core::Resolution { width: v.width, height: v.height };
                        ui.label(egui::RichText::new(res.quality_label())
                            .color(egui::Color32::from_rgb(80, 210, 80)).small());
                        ui.label(egui::RichText::new(
                            format!("{}×{}", v.width, v.height))
                            .small().color(egui::Color32::from_gray(145)));
                        if v.fps > 0.0 {
                            ui.label(egui::RichText::new(format!("{:.0}fps", v.fps))
                                .small().color(egui::Color32::from_gray(130)));
                        }
                        if !v.codec_name.is_empty() {
                            ui.label(egui::RichText::new(v.codec_name.to_uppercase())
                                .small().color(egui::Color32::from_gray(145)));
                        }
                        if v.hdr {
                            ui.label(egui::RichText::new("HDR")
                                .color(egui::Color32::from_rgb(255, 160, 40)).small());
                        }
                    }
                }
            });
        });
    }
}
