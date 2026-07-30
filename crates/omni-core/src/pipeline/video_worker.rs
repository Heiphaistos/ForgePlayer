use crossbeam_channel::{Receiver, Sender};
use ffmpeg_next as ffmpeg;

use crate::decoder::video::VideoDecoder;
use crate::decoder::DecodedVideoFrame;
use crate::pipeline::PipelineEvent;

/// Décodage vidéo sur un thread dédié, séparé du thread demuxer/audio.
///
/// Le décodage logiciel d'un flux 4K/HEVC 10-bit peut prendre plus de temps
/// réel qu'il n'en faut à lire — s'il tournait sur le même thread que la
/// démultiplexion+décodage audio (cas d'avant), l'audio se retrouvait bloqué
/// derrière chaque paquet vidéo lent, d'où les coupures audio observées sur
/// les fichiers 4K/HDR. Ce worker isole ce coût CPU pour que l'audio ne soit
/// jamais affamé par la vidéo.
pub enum VideoWorkerMsg {
    Packet(ffmpeg::Packet),
    /// Remplace le décodeur courant (post-seek) — l'ancien est simplement jeté,
    /// ce qui purge tout état interne (scaler, images de référence) sans avoir
    /// besoin d'un aller-retour send_eof/drain explicite.
    Reset {
        decoder:    Option<VideoDecoder>,
        skip_until: Option<f64>,
    },
    /// Fin de flux : vide les frames restantes du décodeur puis accuse réception.
    Eof,
}

pub fn spawn(
    initial_decoder: Option<VideoDecoder>,
    msg_rx:          Receiver<VideoWorkerMsg>,
    video_tx:        Sender<DecodedVideoFrame>,
    event_tx:        Sender<PipelineEvent>,
    eof_ack_tx:       Sender<()>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("omni-video-decode".into())
        .spawn(move || run(initial_decoder, msg_rx, video_tx, event_tx, eof_ack_tx))
}

fn run(
    mut video_dec: Option<VideoDecoder>,
    msg_rx:        Receiver<VideoWorkerMsg>,
    video_tx:      Sender<DecodedVideoFrame>,
    event_tx:      Sender<PipelineEvent>,
    eof_ack_tx:    Sender<()>,
) {
    let mut v_skip_until: Option<f64> = None;
    // DBGPROBE (diagnostic temporaire, RUST_LOG=debug) : compte paquets/frames
    // pour localiser où la vidéo se perd (pas de décodeur, send_packet en échec,
    // aucune frame produite, ou frames produites mais droppées car video_tx plein).
    let mut n_packets = 0u64;
    let mut n_send_err = 0u64;
    let mut n_frames_decoded = 0u64;
    let mut n_frames_dropped = 0u64;
    log::debug!("DBGPROBE video_worker: démarré, décodeur initial présent={}", video_dec.is_some());

    for msg in msg_rx {
        match msg {
            VideoWorkerMsg::Reset { decoder, skip_until } => {
                log::debug!("DBGPROBE video_worker: Reset décodeur présent={}", decoder.is_some());
                video_dec    = decoder;
                v_skip_until = skip_until;
            }
            VideoWorkerMsg::Packet(packet) => {
                n_packets += 1;
                let Some(dec) = &mut video_dec else { continue };
                if dec.send_packet(&packet).is_err() { n_send_err += 1; continue; }
                while let Ok(Some(frame)) = dec.receive_frame() {
                    n_frames_decoded += 1;
                    if let Some(su) = v_skip_until {
                        if frame.pts_secs < su - 0.05 { continue; }
                        v_skip_until = None;
                    }
                    let _ = event_tx.try_send(PipelineEvent::PositionChanged(frame.pts_secs));
                    if video_tx.try_send(frame).is_err() { n_frames_dropped += 1; }
                }
            }
            VideoWorkerMsg::Eof => {
                if let Some(dec) = &mut video_dec {
                    let _ = dec.send_eof();
                    while let Ok(Some(f)) = dec.receive_frame() {
                        n_frames_decoded += 1;
                        let _ = video_tx.try_send(f);
                    }
                }
                log::debug!(
                    "DBGPROBE video_worker EOF: paquets={n_packets} send_err={n_send_err} \
                     frames_décodées={n_frames_decoded} frames_droppées(video_tx plein)={n_frames_dropped}"
                );
                let _ = eof_ack_tx.try_send(());
            }
        }
    }
}
