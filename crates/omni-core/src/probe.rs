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
    /// Vrai si les échantillons couvrent toute la plage 0-255 (JPEG/PC) au
    /// lieu de la plage limitée 16-235 (MPEG/TV). Décoder du full range avec
    /// les offsets du limited range délave l'image (noirs gris, blancs gris).
    pub full_range: bool,
    /// Pic de luminance réel du contenu en nits, lu dans les métadonnées HDR10
    /// du flux (MaxCLL, sinon la luminance max de l'écran de mastering).
    /// `None` = aucune métadonnée, l'appelant retombe sur sa valeur de config.
    pub peak_nits:  Option<f32>,
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
    // Même traitement que l'ouverture principale : sans options réseau, la
    // sonde bloque sur le délai par défaut de FFmpeg avant de rendre la main.
    let mut ctx = if crate::decoder::context::is_network_url(&path_str) {
        ffmpeg::format::input_with_dictionary(&path, crate::decoder::context::network_options())
            .with_context(|| format!("ouverture de {path_str}"))?
    } else {
        ffmpeg::format::input(&path)
            .with_context(|| format!("ouverture de {path_str}"))?
    };

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
                    let full_range = dec.color_range() == ffmpeg::color::Range::JPEG;
                    let transfer = match dec.color_transfer_characteristic() {
                        ffmpeg::color::TransferCharacteristic::SMPTE2084   => 1u8,
                        ffmpeg::color::TransferCharacteristic::ARIB_STD_B67 => 2u8,
                        _ => 0u8,
                    };
                    let hdr = transfer != 0;
                    let peak_nits = unsafe { hdr_peak_nits(stream.parameters().as_ptr()) };

                    video_info = Some(VideoStreamInfo {
                        index:      stream.index(),
                        codec_name: codec_name.clone(),
                        width:      dec.width(),
                        height:     dec.height(),
                        fps:        fps_f,
                        bit_rate:   dec.bit_rate() as i64,
                        hdr,
                        full_range,
                        peak_nits,
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

    // Le pic HDR10 peut n'exister que dans les SEI du flux (cas d'un encodage
    // x265 `hdr10=1` sans boîtes mdcv/clli au niveau du conteneur) : il
    // n'apparaît alors PAS dans `codecpar`, seulement sur les frames décodées.
    // On décode donc la première image quand le conteneur n'a rien annoncé.
    if let Some(v) = video_info.as_mut() {
        if v.hdr && v.peak_nits.is_none() {
            v.peak_nits = first_frame_peak_nits(&mut ctx, v.index);
        }
        match v.peak_nits {
            Some(p) => log::info!("HDR: transfert={} (1=PQ, 2=HLG), pic annoncé par le fichier = {p:.0} nits", v.transfer),
            None if v.hdr => log::info!("HDR: transfert={} (1=PQ, 2=HLG), aucune métadonnée de pic, repli sur les réglages", v.transfer),
            None => {}
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

/// Métadonnées HDR10 transportées en side data du flux. `libavutil` les publie
/// dans `mastering_display_metadata.h`, que `ffmpeg-sys-next` ne binde pas —
/// les deux structures sont donc redéclarées ici, à l'identique de l'en-tête.
/// Seuls les champs lus ci-dessous comptent, d'éventuels champs ajoutés en fin
/// de structure par une version ultérieure de FFmpeg ne changeraient pas leur
/// position.
#[repr(C)]
struct AvMasteringDisplayMetadata {
    display_primaries: [[ffmpeg::ffi::AVRational; 2]; 3],
    white_point:       [ffmpeg::ffi::AVRational; 2],
    min_luminance:     ffmpeg::ffi::AVRational,
    max_luminance:     ffmpeg::ffi::AVRational,
    has_primaries:     std::os::raw::c_int,
    has_luminance:     std::os::raw::c_int,
}

#[repr(C)]
struct AvContentLightMetadata {
    max_cll:  std::os::raw::c_uint,
    max_fall: std::os::raw::c_uint,
}

/// Pic de luminance du contenu, en nits : MaxCLL s'il est présent (c'est le
/// pic RÉEL de l'image, mesuré à l'encodage), sinon la luminance max de
/// l'écran de mastering (le pic que le coloriste voyait). Tone mapper sur une
/// valeur figée alors que le fichier annonce la sienne écrase inutilement les
/// hautes lumières d'un master 4000 nits, ou en laisse passer trop sur un
/// master 600 nits.
unsafe fn hdr_peak_nits(par: *const ffmpeg::ffi::AVCodecParameters) -> Option<f32> {
    if par.is_null() { return None; }
    let list = (*par).coded_side_data;
    let count = (*par).nb_coded_side_data;
    if list.is_null() || count <= 0 { return None; }

    let mut from_cll: Option<f32> = None;
    let mut from_master: Option<f32> = None;

    for i in 0..count as usize {
        let entry = &*list.add(i);
        match entry.type_ {
            ffmpeg::ffi::AVPacketSideDataType::AV_PKT_DATA_CONTENT_LIGHT_LEVEL => {
                if entry.size >= std::mem::size_of::<AvContentLightMetadata>() {
                    let m = &*(entry.data as *const AvContentLightMetadata);
                    if m.max_cll > 0 { from_cll = Some(m.max_cll as f32); }
                }
            }
            ffmpeg::ffi::AVPacketSideDataType::AV_PKT_DATA_MASTERING_DISPLAY_METADATA => {
                if entry.size >= std::mem::size_of::<AvMasteringDisplayMetadata>() {
                    let m = &*(entry.data as *const AvMasteringDisplayMetadata);
                    if m.has_luminance != 0 && m.max_luminance.den != 0 {
                        from_master = Some(m.max_luminance.num as f32 / m.max_luminance.den as f32);
                    }
                }
            }
            _ => {}
        }
    }

    from_cll.or(from_master).and_then(sane_peak)
}

/// Décode la première image de la piste vidéo pour y lire les métadonnées HDR10
/// portées par les SEI. Borné à 60 paquets : un fichier dont la première image
/// n'arrive pas dans ce budget n'a de toute façon pas de métadonnée exploitable
/// à ce stade, et la sonde ne doit jamais retarder l'ouverture.
fn first_frame_peak_nits(
    ctx:       &mut ffmpeg::format::context::Input,
    video_idx: usize,
) -> Option<f32> {
    let params = ctx.stream(video_idx)?.parameters();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(params)
        .ok()?
        .decoder()
        .video()
        .ok()?;

    let mut frame = ffmpeg::util::frame::video::Video::empty();
    let mut budget = 60;
    for (stream, packet) in ctx.packets() {
        if stream.index() != video_idx { continue; }
        budget -= 1;
        if budget <= 0 { break; }
        if decoder.send_packet(&packet).is_err() { continue; }
        if decoder.receive_frame(&mut frame).is_ok() {
            return unsafe { frame_peak_nits(&frame) };
        }
    }
    None
}

/// Même logique que `hdr_peak_nits`, mais sur les side data d'une frame
/// décodée (SEI du flux) au lieu de celles du conteneur.
unsafe fn frame_peak_nits(frame: &ffmpeg::util::frame::video::Video) -> Option<f32> {
    use ffmpeg::ffi::AVFrameSideDataType::*;

    let cll = ffmpeg::ffi::av_frame_get_side_data(frame.as_ptr(), AV_FRAME_DATA_CONTENT_LIGHT_LEVEL);
    if !cll.is_null() && (*cll).size >= std::mem::size_of::<AvContentLightMetadata>() {
        let m = &*((*cll).data as *const AvContentLightMetadata);
        if m.max_cll > 0 { return sane_peak(m.max_cll as f32); }
    }

    let mdm = ffmpeg::ffi::av_frame_get_side_data(frame.as_ptr(), AV_FRAME_DATA_MASTERING_DISPLAY_METADATA);
    if !mdm.is_null() && (*mdm).size >= std::mem::size_of::<AvMasteringDisplayMetadata>() {
        let m = &*((*mdm).data as *const AvMasteringDisplayMetadata);
        if m.has_luminance != 0 && m.max_luminance.den != 0 {
            return sane_peak(m.max_luminance.num as f32 / m.max_luminance.den as f32);
        }
    }
    None
}

/// Un pic hors de cette plage est une métadonnée cassée : on l'ignore plutôt
/// que de tone mapper n'importe comment.
fn sane_peak(v: f32) -> Option<f32> {
    (v.is_finite() && (100.0..=10000.0).contains(&v)).then_some(v)
}
