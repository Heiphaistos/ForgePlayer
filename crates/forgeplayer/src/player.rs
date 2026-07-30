use anyhow::{Context as _, Result};
use omni_core::decoder::subtitle::SubtitleTrack;
use omni_core::decoder::DecodedVideoFrame;
use omni_core::pipeline::clock::MasterClock;
use omni_core::pipeline::{MediaPipeline, PipelineCommand, PipelineEvent};
use omni_core::probe::{Chapter, MediaInfo, VideoStreamInfo};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerState {
    Idle,
    Loading,
    Playing,
    Paused,
    Buffering(u8),
    EndOfFile,
    Error(String),
}

/// Frame image statique (RGBA8) pour le mode visionneuse.
pub struct ImageFrame {
    pub width:  u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub path:   String,
}

pub struct Player {
    pub state:            PlayerState,
    pub duration:         f64,
    pub position:         f64,
    pub volume:           f32,
    pub muted:            bool,
    pub media_info:       Option<MediaInfo>,
    pub subtitle_track:   Option<SubtitleTrack>,
    pub current_subtitle: Option<String>,
    pub chapters:         Vec<Chapter>,
    pub audio_track_idx:  usize,
    pub sub_track_idx:    Option<usize>,
    pub clock:            MasterClock,
    pub image_frame:      Option<ImageFrame>,
    /// Sous-titres intégrés décodés en avance : (piste, texte, pts_start, pts_end).
    /// Toutes les pistes sont conservées — le filtrage se fait à l'affichage.
    embedded_events:      Vec<(usize, String, f64, f64)>,
    /// Signale à l'app que le moteur audio doit être purgé (seek / nouveau fichier).
    pub audio_flush_needed: bool,
    /// true = l'horloge est pilotée par la position audio réellement jouée
    /// (app::sync_clock_to_audio). Les PositionChanged du pipeline (position de
    /// DÉCODAGE, en avance de tout le buffer) ne pilotent alors plus l'horloge.
    pub clock_audio_master: bool,
    pipeline:             Option<MediaPipeline>,
}

impl Player {
    pub fn new() -> Self {
        Self {
            state:            PlayerState::Idle,
            duration:         0.0,
            position:         0.0,
            volume:           1.0,
            muted:            false,
            media_info:       None,
            subtitle_track:   None,
            current_subtitle: None,
            chapters:         Vec::new(),
            audio_track_idx:  0,
            sub_track_idx:    None,
            clock:            MasterClock::new(),
            image_frame:      None,
            embedded_events:  Vec::new(),
            audio_flush_needed: false,
            clock_audio_master: false,
            pipeline:         None,
        }
    }

    pub fn open(&mut self, path: &str) -> Result<()> {
        if let Some(p) = &self.pipeline { p.send_command(PipelineCommand::Stop); }
        self.pipeline         = None;
        self.state            = PlayerState::Loading;
        self.duration         = 0.0;
        self.position         = 0.0;
        self.media_info       = None;
        self.subtitle_track   = None;
        self.current_subtitle = None;
        self.chapters         = Vec::new();
        self.audio_track_idx  = 0;
        self.sub_track_idx    = None;
        self.image_frame      = None;
        self.embedded_events.clear();
        self.audio_flush_needed = true;
        self.clock            = MasterClock::new();

        if omni_core::is_image_path(path) {
            return self.open_image(path);
        }

        self.pipeline = Some(MediaPipeline::launch(path.to_string())?);
        Ok(())
    }

    fn open_image(&mut self, path: &str) -> Result<()> {
        let img = image::open(path)
            .with_context(|| format!("image non lisible : {path}"))?
            .into_rgba8();
        let (width, height) = img.dimensions();
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("image")
            .to_uppercase();
        let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

        self.image_frame = Some(ImageFrame {
            width, height, pixels: img.into_raw(), path: path.to_string(),
        });
        self.media_info = Some(MediaInfo {
            path:          path.to_string(),
            duration_secs: 0.0,
            video: Some(VideoStreamInfo {
                index:       0,
                codec_name:  ext,
                width, height,
                fps:         0.0,
                bit_rate:    file_size as i64,
                hdr:         false,
                color_space: "sRGB".to_string(),
            }),
            audio:     vec![],
            subtitles: vec![],
            chapters:  vec![],
            format_name: "image".to_string(),
            bit_rate:    file_size as i64,
        });
        self.state = PlayerState::Paused;
        Ok(())
    }

    pub fn is_image_mode(&self) -> bool {
        self.image_frame.is_some()
    }

    pub fn play_pause(&mut self) {
        match &self.state {
            PlayerState::Playing => {
                if let Some(p) = &self.pipeline { p.send_command(PipelineCommand::Pause); }
                self.clock.pause();
                self.state = PlayerState::Paused;
            }
            PlayerState::Paused if !self.is_image_mode() => {
                if let Some(p) = &self.pipeline { p.send_command(PipelineCommand::Resume); }
                self.clock.resume();
                self.state = PlayerState::Playing;
            }
            // Fin de fichier : le pipeline est terminé — on relance depuis le début
            PlayerState::EndOfFile => { self.replay(); }
            _ => {}
        }
    }

    /// Relance le média courant depuis le début (pipeline terminé après EOF).
    /// Préserve le sous-titre externe chargé. Retourne false si aucun média.
    pub fn replay(&mut self) -> bool {
        let Some(path) = self.media_info.as_ref().map(|i| i.path.clone()) else { return false };
        let subs = self.subtitle_track.take();
        let ok = self.open(&path).is_ok();
        if ok { self.subtitle_track = subs; }
        ok
    }

    pub fn seek(&mut self, pos: f64) {
        if self.is_image_mode() { return; }
        let pos = pos.clamp(0.0, self.duration.max(0.0));
        // Pipeline terminé (EOF) : relancer puis positionner
        if self.state == PlayerState::EndOfFile {
            if !self.replay() { return; }
        }
        if let Some(p) = &self.pipeline {
            p.send_command(PipelineCommand::Seek(pos));
            // Purge les frames pré-seek déjà décodées en file (jusqu'à ~10 s d'audio
            // et 16 frames vidéo) — sinon elles seraient rejouées après le seek
            while p.try_recv_audio_frame().is_some() {}
            while p.try_recv_video_frame().is_some() {}
        }
        self.position = pos;
        self.clock.seek(pos);
        self.embedded_events.clear();
        self.current_subtitle = None;
        self.audio_flush_needed = true;
    }

    pub fn seek_relative(&mut self, delta: f64) {
        self.seek(self.position + delta);
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 2.0);
        let effective = if self.muted { 0.0 } else { self.volume };
        if let Some(p) = &self.pipeline { p.send_command(PipelineCommand::SetVolume(effective)); }
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        let effective = if self.muted { 0.0 } else { self.volume };
        if let Some(p) = &self.pipeline { p.send_command(PipelineCommand::SetVolume(effective)); }
    }

    pub fn stop(&mut self) {
        if let Some(p) = &self.pipeline { p.send_command(PipelineCommand::Stop); }
        self.pipeline    = None;
        self.image_frame = None;
        self.state       = PlayerState::Idle;
        self.position    = 0.0;
    }

    pub fn load_subtitle(&mut self, path: &str) -> Result<()> {
        let data = std::fs::read_to_string(path)?;
        let track = if path.ends_with(".ass") || path.ends_with(".ssa") {
            SubtitleTrack::from_ass(&data)?
        } else {
            SubtitleTrack::from_srt(&data)?
        };
        self.subtitle_track = Some(track);
        Ok(())
    }

    pub fn clear_subtitle(&mut self) {
        self.subtitle_track   = None;
        self.current_subtitle = None;
    }

    pub fn next_audio_track(&mut self) {
        if let Some(info) = &self.media_info {
            if !info.audio.is_empty() {
                self.audio_track_idx = (self.audio_track_idx + 1) % info.audio.len();
                if let Some(p) = &self.pipeline {
                    p.send_command(PipelineCommand::SelectAudioTrack(self.audio_track_idx));
                }
            }
        }
    }

    pub fn next_subtitle_track(&mut self) {
        if let Some(info) = &self.media_info {
            let n = info.subtitles.len();
            if n > 0 {
                let next = match self.sub_track_idx {
                    None              => Some(0),
                    Some(i) if i + 1 < n => Some(i + 1),
                    _                 => None,
                };
                self.sub_track_idx = next;
                if next.is_none() { self.current_subtitle = None; }
                if let Some(p) = &self.pipeline {
                    p.send_command(PipelineCommand::SelectSubtitleTrack(self.sub_track_idx));
                }
            }
        }
    }

    pub fn chapter_prev(&mut self) {
        if self.chapters.is_empty() { return; }
        let pos = self.position;
        let t = self.chapters.iter().rev()
            .find(|c| c.start_secs < pos - 2.0)
            .map(|c| c.start_secs)
            .unwrap_or(0.0);
        self.seek(t);
    }

    pub fn chapter_next(&mut self) {
        if self.chapters.is_empty() { return; }
        let pos = self.position;
        if let Some(t) = self.chapters.iter().find(|c| c.start_secs > pos).map(|c| c.start_secs) {
            self.seek(t);
        }
    }

    pub fn try_recv_video_frame(&self) -> Option<DecodedVideoFrame> {
        self.pipeline.as_ref()?.try_recv_video_frame()
    }

    pub fn try_recv_audio_frame(&self) -> Option<omni_core::decoder::DecodedAudioFrame> {
        self.pipeline.as_ref()?.try_recv_audio_frame()
    }

    pub fn display_title(&self) -> Option<String> {
        self.media_info.as_ref().map(|i| {
            std::path::Path::new(&i.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| i.path.clone())
        })
    }

    pub fn poll_events(&mut self) {
        if self.is_image_mode() { return; }
        let Some(pipeline) = &self.pipeline else { return };

        while let Some(event) = pipeline.try_recv_event() {
            match event {
                PipelineEvent::DurationKnown(d) => { self.duration = d; }
                PipelineEvent::PositionChanged(p) => {
                    // p = position de décodage, pas de lecture réelle (en avance de
                    // tout le buffer). Ne sert qu'à AMORCER l'horloge au démarrage
                    // (ou après un seek) — une fois lancée, l'horloge tourne en
                    // temps réel (wall-clock, via clock.resume()) et ne doit PAS
                    // être re-pilotée par chaque nouvelle frame décodée : sinon la
                    // position affichée == vitesse de décodage, pas le temps réel.
                    // Sans audio pour la piloter en continu (voir sync_clock_to_audio
                    // dans app.rs), un décodage plus lent que le temps réel (ex.
                    // rendu logiciel en VM sans GPU) ferait dériver la lecture en
                    // retard indéfiniment au lieu de sauter les frames en retard.
                    if self.state == PlayerState::Loading {
                        self.position = p;
                        self.clock.update(p);
                        self.state = PlayerState::Playing;
                        self.clock.resume();
                    }
                }
                PipelineEvent::BufferingProgress(b) => { self.state = PlayerState::Buffering(b); }
                PipelineEvent::EndOfStream          => { self.state = PlayerState::EndOfFile; }
                PipelineEvent::Error(e)             => { self.state = PlayerState::Error(e); }
                PipelineEvent::MetadataReady(info)  => {
                    self.chapters   = info.chapters.clone();
                    self.media_info = Some(*info);
                }
                PipelineEvent::SubtitleLine(track, text, start, end) => {
                    // Les paquets sont décodés en avance sur la lecture : on met en file
                    // et update_subtitle() affiche au bon PTS (filtré par piste active).
                    if !text.is_empty() {
                        self.embedded_events.push((track, text, start, end));
                        // Garde-fou mémoire : ne conserve que les 512 derniers événements
                        if self.embedded_events.len() > 512 {
                            self.embedded_events.remove(0);
                        }
                    }
                }
            }
        }

        self.update_subtitle();
    }

    fn update_subtitle(&mut self) {
        // Sous-titre externe (fichier .srt/.ass chargé) : prioritaire
        if let Some(t) = &self.subtitle_track {
            let pos = Duration::from_secs_f64(self.position.max(0.0));
            self.current_subtitle = t.events_at(pos).next().map(|e| e.text.clone());
            return;
        }
        // Sous-titres intégrés : affiche l'événement de la piste active couvrant
        // la position courante
        if let Some(track) = self.sub_track_idx {
            let pos = self.position;
            self.current_subtitle = self.embedded_events.iter()
                .find(|(tr, _, s, e)| *tr == track && *s <= pos && pos <= *e)
                .map(|(_, t, _, _)| t.clone());
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state,
            PlayerState::Playing | PlayerState::Paused | PlayerState::Buffering(_))
    }

    pub fn effective_volume(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume }
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.clock.set_speed(speed);
    }

    pub fn speed(&self) -> f32 {
        self.clock.speed()
    }
}
