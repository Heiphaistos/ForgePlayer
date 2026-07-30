use anyhow::{Context as _, Result};
use ffmpeg_next as ffmpeg;
use ffmpeg::software::scaling::{context::Context as SwsContext, flag::Flags};

use super::PixelFormat;

/// Frame vidéo décodée, prête à envoyer au renderer.
#[derive(Clone)]
pub struct DecodedVideoFrame {
    /// Timestamp de présentation en secondes.
    pub pts_secs:  f64,
    pub width:     u32,
    pub height:    u32,
    pub format:    PixelFormat,
    /// Plans vidéo: [Y, U, V] ou [Y+UV pour NV12] ou [RGBA unique].
    pub planes:    Vec<Vec<u8>>,
    /// Strides (bytes par ligne) par plan.
    pub strides:   Vec<usize>,
}

/// Décodeur vidéo avec gestion du scaling/conversion de format.
pub struct VideoDecoder {
    decoder:     ffmpeg::codec::decoder::Video,
    scaler:      Option<SwsContext>,
    time_base:   f64,
    // Tracks source properties to detect mid-stream changes requiring scaler rebuild.
    scaler_src_w:   u32,
    scaler_src_h:   u32,
    scaler_src_fmt: Option<ffmpeg::format::Pixel>,
    scaler_target_fmt: Option<ffmpeg::format::Pixel>,
}

impl VideoDecoder {
    pub fn new(decoder: ffmpeg::codec::decoder::Video, time_base: f64) -> Result<Self> {
        Ok(Self {
            decoder,
            scaler: None,
            time_base,
            scaler_src_w:   0,
            scaler_src_h:   0,
            scaler_src_fmt: None,
            scaler_target_fmt: None,
        })
    }

    /// Format cible pour le scaler : préserve les 10 bits pour les sources HDR
    /// (PQ/HLG déjà en 10-bit) au lieu de tout tronquer en 8-bit YUV420P — sinon
    /// le HDR est décodé mais toujours écrasé en SDR avant même le rendu.
    /// Tout le reste (immense majorité SDR 8-bit) va en YUV420P, inchangé.
    fn desired_target(src: ffmpeg::format::Pixel) -> ffmpeg::format::Pixel {
        use ffmpeg::format::Pixel::*;
        match src {
            YUV420P10LE | YUV420P10BE
            | YUV422P10LE | YUV422P10BE
            | YUV444P10LE | YUV444P10BE
            | P010LE | P010BE => YUV420P10LE,
            _ => YUV420P,
        }
    }

    /// Envoie un paquet compressé au décodeur.
    pub fn send_packet(&mut self, packet: &ffmpeg::Packet) -> Result<()> {
        self.decoder
            .send_packet(packet)
            .context("send_packet vidéo")
    }

    /// Envoie le signal de fin de flux.
    pub fn send_eof(&mut self) -> Result<()> {
        self.decoder.send_eof().context("send_eof vidéo")
    }

    /// Reçoit une frame décodée si disponible.
    pub fn receive_frame(&mut self) -> Result<Option<DecodedVideoFrame>> {
        let mut raw = ffmpeg::util::frame::video::Video::empty();
        match self.decoder.receive_frame(&mut raw) {
            Ok(()) => {}
            Err(ffmpeg::Error::Other { errno: ffmpeg::error::EAGAIN }) => return Ok(None),
            Err(e) => return Err(e).context("receive_frame vidéo"),
        }

        let pts_secs = raw
            .pts()
            .map(|p| p as f64 * self.time_base)
            .unwrap_or(0.0);

        // Frame décodée sur GPU (D3D11VA/DXVA2) : rapatrie en mémoire système
        // avant toute conversion — le reste du pipeline (extract_planes,
        // SwsContext) ne connaît que des frames logicielles. `pts_secs` a déjà
        // été lu ci-dessus : av_hwframe_transfer_data ne copie pas le PTS, pas
        // besoin de le relire sur la frame téléchargée.
        let raw = if is_hw_format(raw.format()) {
            match download_hw_frame(&raw) {
                Ok(sw) => sw,
                Err(e) => {
                    log::warn!("téléchargement frame GPU échoué, frame ignorée: {e:#}");
                    return Ok(None);
                }
            }
        } else {
            raw
        };

        // Conversion de format si nécessaire (ex: yuv420p10le nvidia → yuv420p10le
        // uniforme, ou tout format exotique → yuv420p)
        let target_fmt = Self::desired_target(raw.format());
        let frame = if raw.format() != target_fmt {
            // Rebuild scaler if source dimensions, pixel format, or target changed.
            let needs_rebuild = self.scaler.is_none()
                || self.scaler_src_w   != raw.width()
                || self.scaler_src_h   != raw.height()
                || self.scaler_src_fmt != Some(raw.format())
                || self.scaler_target_fmt != Some(target_fmt);

            if needs_rebuild {
                self.scaler = Some(
                    SwsContext::get(
                        raw.format(),
                        raw.width(),
                        raw.height(),
                        target_fmt,
                        raw.width(),
                        raw.height(),
                        // Dimensions inchangées ici (seul le format pixel change, ex.
                        // 4:2:2/4:4:4→4:2:0 ou exotique→yuv420p) — BICUBIC coûte à peine
                        // plus que BILINEAR à cette taille et évite le flou chroma
                        // perceptible sur les masters 4K haute qualité.
                        Flags::BICUBIC,
                    )
                    .context("création SwsContext — format/dimensions incompatibles")?,
                );
                self.scaler_src_w   = raw.width();
                self.scaler_src_h   = raw.height();
                self.scaler_src_fmt = Some(raw.format());
                self.scaler_target_fmt = Some(target_fmt);
            }

            let scaler = self.scaler.as_mut().expect("scaler vient d'être initialisé");
            let mut converted = ffmpeg::util::frame::video::Video::empty();
            scaler.run(&raw, &mut converted)?;
            converted
        } else {
            raw
        };

        let (planes, strides, format) = extract_planes(&frame);

        Ok(Some(DecodedVideoFrame {
            pts_secs,
            width:   frame.width(),
            height:  frame.height(),
            format,
            planes,
            strides,
        }))
    }

    pub fn width(&self)  -> u32 { self.decoder.width() }
    pub fn height(&self) -> u32 { self.decoder.height() }
}

fn extract_planes(frame: &ffmpeg::util::frame::video::Video) -> (Vec<Vec<u8>>, Vec<usize>, PixelFormat) {
    match frame.format() {
        ffmpeg::format::Pixel::YUV420P => {
            let (_w, h) = (frame.width() as usize, frame.height() as usize);
            let y_stride  = frame.stride(0);
            let uv_stride = frame.stride(1);

            let y = frame.data(0)[..y_stride * h].to_vec();
            let u = frame.data(1)[..uv_stride * (h / 2)].to_vec();
            let v = frame.data(2)[..uv_stride * (h / 2)].to_vec();

            (vec![y, u, v], vec![y_stride, uv_stride, uv_stride], PixelFormat::Yuv420p)
        }
        ffmpeg::format::Pixel::YUV420P10LE => {
            let (_w, h) = (frame.width() as usize, frame.height() as usize);
            let y_stride  = frame.stride(0);
            let uv_stride = frame.stride(1);

            // FFmpeg stocke le 10-bit dans les bits BAS de chaque mot 16-bit.
            // On décale de 6 bits vers les bits HAUTS (convention P010) pour que
            // la normalisation automatique R16Unorm du shader (valeur/65535)
            // retombe exactement sur le même ratio noir/blanc/plage limitée
            // qu'en 8-bit — aucun changement de shader nécessaire.
            let y = shift_10_to_16(&frame.data(0)[..y_stride * h]);
            let u = shift_10_to_16(&frame.data(1)[..uv_stride * (h / 2)]);
            let v = shift_10_to_16(&frame.data(2)[..uv_stride * (h / 2)]);

            (vec![y, u, v], vec![y_stride, uv_stride, uv_stride], PixelFormat::Yuv420p10le)
        }
        ffmpeg::format::Pixel::NV12 => {
            let (_w, h) = (frame.width() as usize, frame.height() as usize);
            let y_stride  = frame.stride(0);
            let uv_stride = frame.stride(1);
            let y  = frame.data(0)[..y_stride * h].to_vec();
            let uv = frame.data(1)[..uv_stride * (h / 2)].to_vec();
            (vec![y, uv], vec![y_stride, uv_stride], PixelFormat::Nv12)
        }
        _ => {
            // Fallback: data du plan 0 en RGBA (après conversion SwsContext)
            let stride = frame.stride(0);
            let data   = frame.data(0)[..stride * frame.height() as usize].to_vec();
            (vec![data], vec![stride], PixelFormat::Rgba)
        }
    }
}

fn is_hw_format(fmt: ffmpeg::format::Pixel) -> bool {
    matches!(fmt, ffmpeg::format::Pixel::D3D11 | ffmpeg::format::Pixel::DXVA2_VLD)
}

/// Copie les données pixel d'une frame GPU (D3D11/DXVA2) vers une frame
/// mémoire système. Format de sortie non fixé (`Video::empty()` laisse
/// `format = AV_PIX_FMT_NONE`) : FFmpeg choisit automatiquement le format
/// natif du hwframe (NV12 8-bit ou P010LE 10-bit selon la source).
fn download_hw_frame(
    src: &ffmpeg::util::frame::video::Video,
) -> Result<ffmpeg::util::frame::video::Video> {
    let mut sw = ffmpeg::util::frame::video::Video::empty();
    let ret = unsafe {
        ffmpeg::ffi::av_hwframe_transfer_data(sw.as_mut_ptr(), src.as_ptr(), 0)
    };
    if ret < 0 {
        anyhow::bail!("av_hwframe_transfer_data a échoué (code {ret})");
    }
    Ok(sw)
}

/// Décale chaque échantillon 16-bit little-endian de 6 bits vers la gauche :
/// convertit la convention FFmpeg 10LE (valeur dans les bits bas) vers la
/// convention P010 (valeur dans les bits hauts) attendue par le shader.
fn shift_10_to_16(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks_exact(2) {
        let v = u16::from_le_bytes([chunk[0], chunk[1]]);
        out.extend_from_slice(&(v << 6).to_le_bytes());
    }
    out
}
