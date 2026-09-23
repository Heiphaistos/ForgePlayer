pub mod clock;
pub mod demuxer;
pub mod video_worker;

use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::thread;

use crate::decoder::{DecodedAudioFrame, DecodedVideoFrame};

/// Commandes envoyées au thread de pipeline.
#[derive(Debug)]
pub enum PipelineCommand {
    /// Vitesse de lecture : le son est étiré par `atempo` (hauteur conservée).
    SetSpeed(f32),
    Pause,
    Resume,
    Seek(f64),          // position en secondes
    SetVolume(f32),     // 0.0–1.0
    SelectAudioTrack(usize),
    SelectSubtitleTrack(Option<usize>),
    Stop,
}

/// Événements émis par le pipeline vers l'UI.
#[derive(Debug)]
pub enum PipelineEvent {
    PositionChanged(f64),    // secondes
    DurationKnown(f64),
    BufferingProgress(u8),   // 0–100
    EndOfStream,
    Error(String),
    /// Problème non-fatal (ex: piste audio illisible) — la lecture continue en
    /// mode dégradé, l'UI affiche juste un avis transitoire (OSD).
    Warning(String),
    MetadataReady(Box<crate::probe::MediaInfo>),
    /// Ligne de sous-titre intégrée (ordinal piste, texte, pts_start, pts_end en secondes).
    /// Toutes les pistes texte sont décodées ; le player filtre par piste active.
    SubtitleLine(usize, String, f64, f64),
    /// Images de sous-titre (PGS/VOBSUB/DVB) : ordinal de piste, rectangles
    /// RGBA, pts_start, pts_end en secondes. Un cue peut contenir plusieurs
    /// rectangles (texte principal + incrustation).
    SubtitleBitmap(usize, Vec<crate::decoder::subtitle::SubtitleBitmap>, f64, f64),
}

/// Capacité max des queues de frames (frames buffered).
///
/// Une image 4K 10 bits pèse ~25 Mo décodée : seize d'avance immobilisent
/// 400 Mo pour deux tiers de seconde de lecture, sans rien apporter puisque
/// le worker applique une contre-pression et que l'audio garde plusieurs
/// secondes d'avance. Six images (un quart de seconde) suffisent à absorber
/// les à-coups de décodage.
const VIDEO_QUEUE_DEPTH: usize = 6;
const AUDIO_QUEUE_DEPTH: usize = 512;

pub struct MediaPipeline {
    cmd_tx:   Sender<PipelineCommand>,
    event_rx: Receiver<PipelineEvent>,
    video_rx: Receiver<DecodedVideoFrame>,
    audio_rx: Receiver<DecodedAudioFrame>,
}

impl MediaPipeline {
    /// Lance le pipeline de décodage dans des threads dédiés.
    /// `hw_accel_pref` : "auto"/"d3d11va"/"dxva2"/"none" (réglage Paramètres).
    pub fn launch(
        path: String,
        hw_accel_pref: String,
        zero_copy: bool,
        d3d11_device: usize,
    ) -> Result<Self> {
        let (cmd_tx, cmd_rx)         = bounded::<PipelineCommand>(16);
        let (event_tx, event_rx)     = bounded::<PipelineEvent>(64);
        let (video_tx, video_rx)     = bounded::<DecodedVideoFrame>(VIDEO_QUEUE_DEPTH);
        let (audio_tx, audio_rx)     = bounded::<DecodedAudioFrame>(AUDIO_QUEUE_DEPTH);

        let path_clone = path.clone();
        let event_tx_err = event_tx.clone();
        thread::Builder::new()
            .name("omni-demuxer".into())
            .spawn(move || {
                if let Err(e) = demuxer::run_demuxer(
                    &path_clone, &hw_accel_pref, zero_copy, d3d11_device,
                    cmd_rx, event_tx, video_tx, audio_tx,
                ) {
                    log::error!("demuxer: {e:#}");
                    // Remonte l'erreur à l'UI — sinon le player reste bloqué en Playing
                    let _ = event_tx_err.try_send(PipelineEvent::Error(format!("{e:#}")));
                }
            })?;

        Ok(Self { cmd_tx, event_rx, video_rx, audio_rx })
    }

    pub fn send_command(&self, cmd: PipelineCommand) {
        let _ = self.cmd_tx.try_send(cmd);
    }

    pub fn try_recv_event(&self) -> Option<PipelineEvent> {
        self.event_rx.try_recv().ok()
    }

    pub fn try_recv_video_frame(&self) -> Option<DecodedVideoFrame> {
        self.video_rx.try_recv().ok()
    }

    pub fn try_recv_audio_frame(&self) -> Option<DecodedAudioFrame> {
        self.audio_rx.try_recv().ok()
    }
}
