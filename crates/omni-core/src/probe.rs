use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;
use std::path::Path;

/// Informations extraites d'un fichier média sans décodage complet.
#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub path:          String,
    pub duration_secs: f64,
    pub video:         Option<VideoStreamInfo>,
    pub audio:         Vec<AudioStreamInfo>,
    pub subtitles:     Vec<SubtitleStreamInfo>,
    pub chapters:      Vec<Chapter>,
    pub format_name:   String,
    pub bit_rate:      i64,
}

#[derive(Debug, Clone)]
pub struct VideoStreamInfo {
    pub index:      usize,
    pub codec_name: String,
    pub width:      u32,
    pub height:     u32,
    pub fps:        f64,
    pub bit_rate:   i64,
    pub hdr:        bool,
    /// Fonction de transfert : 0 = SDR, 1 = PQ (SMPTE ST.2084), 2 = HLG.
    /// C'est elle, et non la profondeur de bits, qui détermine si le rendu
    /// doit passer par le tone mapping — un flux 10-bit BT.709 reste du SDR.
    pub transfer:   u8,
    pub color_space: String,
}

#[derive(Debug, Clone)]
pub struct AudioStreamInfo {
    pub index:       usize,
    pub codec_name:  String,
    pub channels:    u16,
    pub sample_rate: u32,
    pub bit_rate:    i64,
    pub language:    String,
}

#[derive(Debug, Clone)]
pub struct SubtitleStreamInfo {
    pub index:    usize,
    pub codec:    String,
    pub language: String,
    pub title:    String,
}

#[derive(Debug, Clone)]
pub struct Chapter {
    pub title:      String,
    pub start_secs: f64,
    pub end_secs:   f64,
}

/// Sonde un fichier et retourne ses métadonnées complètes.
pub fn probe_file(path: &Path) -> Result<MediaInfo> {
    ffmpeg::init().context("ffmpeg init")?;

    let path_str = path.to_string_lossy().to_string();
    let ctx = ffmpeg::format::input(&path)
        .with_context(|| format!("ouverture de {path_str}"))?;

    let format_name = ctx.format().name().to_string();
    let duration_secs = ctx.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);
    let bit_rate = ctx.bit_rate();

    let mut video_info = None;
    let mut audio_streams = Vec::new();
    let mut subtitle_streams = Vec::new();

    for stream in ctx.streams() {
        let params = stream.parameters();
        let codec_id = params.id();
        let codec_name = codec_id.name().to_string();
        let lang = stream
            .metadata()
            .get("language")
            .unwrap_or("und")
            .to_string();

        match params.medium() {
            ffmpeg::media::Type::Video if video_info.is_none() => {
                let decoder = ffmpeg::codec::context::Context::from_parameters(params)
                    .and_then(|c| c.decoder().video())
                    .ok();

                if let Some(dec) = decoder {
                    let fps = stream.avg_frame_rate();
                    let fps_f = fps.numerator() as f64 / fps.denominator().max(1) as f64;

                    // Détection HDR via color space / color transfer
                    let color_space = format!("{:?}", dec.color_space());
                    let transfer = match dec.color_transfer_characteristic() {
                        ffmpeg::color::TransferCharacteristic::SMPTE2084   => 1u8,
                        ffmpeg::color::TransferCharacteristic::ARIB_STD_B67 => 2u8,
                        _ => 0u8,
                    };
                    let hdr = transfer != 0;

                    video_info = Some(VideoStreamInfo {
                        index:      stream.index(),
                        codec_name: codec_name.clone(),
                        width:      dec.width(),
                        height:     dec.height(),
                        fps:        fps_f,
                        bit_rate:   dec.bit_rate() as i64,
                        hdr,
                        transfer,
                        color_space,
                    });
                }
            }
            ffmpeg::media::Type::Audio => {
                let audio_dec = ffmpeg::codec::context::Context::from_parameters(params)
                    .and_then(|c| c.decoder().audio())
                    .ok();
                let channels    = audio_dec.as_ref().map(|d| d.channels() as u16).unwrap_or(0);
                let sample_rate = audio_dec.as_ref().map(|d| d.rate()).unwrap_or(0);
                let bit_rate    = audio_dec.as_ref().map(|d| d.bit_rate() as i64).unwrap_or(0);
                audio_streams.push(AudioStreamInfo {
                    index: stream.index(),
                    codec_name: codec_name.clone(),
                    channels,
                    sample_rate,
                    bit_rate,
                    language: lang,
                });
            }
            ffmpeg::media::Type::Subtitle => {
                let title = stream
                    .metadata()
                    .get("title")
                    .unwrap_or("")
                    .to_string();
                subtitle_streams.push(SubtitleStreamInfo {
                    index: stream.index(),
                    codec: codec_name,
                    language: lang,
                    title,
                });
            }
            _ => {}
        }
    }

    let chapters = ctx
        .chapters()
        .map(|ch| Chapter {
            title:      ch.metadata().get("title").unwrap_or("").to_string(),
            start_secs: ch.start() as f64 * ch.time_base().numerator() as f64
                / ch.time_base().denominator().max(1) as f64,
            end_secs:   ch.end() as f64 * ch.time_base().numerator() as f64
                / ch.time_base().denominator().max(1) as f64,
        })
        .collect();

    Ok(MediaInfo {
        path: path_str,
        duration_secs,
        video: video_info,
        audio: audio_streams,
        subtitles: subtitle_streams,
        chapters,
        format_name,
        bit_rate,
    })
}
