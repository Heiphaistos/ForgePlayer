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
    /// Frame système réutilisée pour le rapatriement GPU : `av_hwframe_transfer_data`
    /// alloue sinon 24 Mo à chaque image en 4K, et cette allocation se paie.
    sw_frame: ffmpeg::util::frame::video::Video,
    prof_n:  u64,
    prof_dl: f64,
    prof_ex: f64,
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
            sw_frame: ffmpeg::util::frame::video::Video::empty(),
            prof_n: 0, prof_dl: 0.0, prof_ex: 0.0,
        })
    }

    /// Format cible pour le scaler. Règle : ne JAMAIS convertir ce que le GPU
    /// sait déjà afficher. NV12 et P010LE (sorties natives du décodage
    /// matériel D3D11VA/DXVA2) partent directement au renderer, comme le fait
    /// VLC — une conversion swscale d'une image 4K coûte plusieurs
    /// millisecondes par frame sur un seul cœur, soit la cause directe des
    /// saccades 4K/2K. Le 10-bit planaire reste préservé en 10-bit ; tout le
    /// reste (formats exotiques) retombe en YUV420P.
    fn desired_target(src: ffmpeg::format::Pixel) -> ffmpeg::format::Pixel {
        use ffmpeg::format::Pixel::*;
        match src {
            // Déjà affichables tels quels : zéro conversion.
            YUV420P | NV12 | P010LE => src,
            YUV420P10LE | YUV420P10BE
            | YUV422P10LE | YUV422P10BE
            | YUV444P10LE | YUV444P10BE
            | P010BE => YUV420P10LE,
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
        let t_dl = std::time::Instant::now();
        // La frame système de destination est réutilisée d'une image à l'autre :
        // laissée vide, `av_hwframe_transfer_data` réalloue 24 Mo par image en
        // 4K. On la sort de `self` le temps du traitement pour ne pas bloquer
        // l'emprunt du scaler, puis on la remet en place.
        let mut sw = std::mem::replace(
            &mut self.sw_frame,
            ffmpeg::util::frame::video::Video::empty(),
        );
        let is_hw = is_hw_format(raw.format());
        if is_hw {
            if let Err(e) = download_hw_frame_into(&mut sw, &raw) {
                log::warn!("téléchargement frame GPU échoué, frame ignorée: {e:#}");
                self.sw_frame = sw;
                return Ok(None);
            }
        }
        let raw: &ffmpeg::util::frame::video::Video = if is_hw { &sw } else { &raw };

        // Conversion de format si nécessaire (ex: yuv420p10le nvidia → yuv420p10le
        // uniforme, ou tout format exotique → yuv420p)
        let target_fmt = Self::desired_target(raw.format());
        let converted_frame;
        let frame: &ffmpeg::util::frame::video::Video = if raw.format() != target_fmt {
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
            if let Err(e) = scaler.run(raw, &mut converted) {
                self.sw_frame = sw;
                return Err(e).context("conversion de format");
            }
            converted_frame = converted;
            &converted_frame
        } else {
            raw
        };

        let dl_ms = t_dl.elapsed().as_secs_f64() * 1000.0;
        let t_ex = std::time::Instant::now();
        let (planes, strides, format) = extract_planes(frame);
        let ex_ms = t_ex.elapsed().as_secs_f64() * 1000.0;
        self.prof_n += 1;
        self.prof_dl += dl_ms;
        self.prof_ex += ex_ms;
        if self.prof_n % 48 == 0 {
            log::debug!("DBGPERF decode: rapatriement GPU {:.2} ms/frame, extraction plans {:.2} ms/frame (sur {} frames)",
                self.prof_dl / self.prof_n as f64, self.prof_ex / self.prof_n as f64, self.prof_n);
        }

        let out = DecodedVideoFrame {
            pts_secs,
            width:   frame.width(),
            height:  frame.height(),
            format,
            planes,
            strides,
        };
        self.sw_frame = sw;
        Ok(Some(out))
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
            // Les échantillons restent tels que FFmpeg les produit : valeur
            // 10 bits dans les bits BAS de chaque mot 16 bits. Aucune passe
            // de décalage ici — le shader remet l'échelle (×65535/1023) d'un
            // multiplication gratuite, là où ce décalage coûtait une relecture
            // et une réallocation de tout le plan (24 Mo par image en 4K).
            let h = frame.height() as usize;
            let y_stride  = frame.stride(0);
            let uv_stride = frame.stride(1);
            let y = frame.data(0)[..y_stride * h].to_vec();
            let u = frame.data(1)[..uv_stride * (h / 2)].to_vec();
            let v = frame.data(2)[..uv_stride * (h / 2)].to_vec();
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
        ffmpeg::format::Pixel::P010LE => {
            // Sortie native du décodage matériel 10-bit : Y 16-bit + UV
            // entrelacé 16-bit, valeurs déjà alignées sur les bits hauts.
            // Aucun décalage, aucune conversion — upload direct.
            let h = frame.height() as usize;
            let y_stride  = frame.stride(0);
            let uv_stride = frame.stride(1);
            let y  = frame.data(0)[..y_stride * h].to_vec();
            let uv = frame.data(1)[..uv_stride * (h / 2)].to_vec();
            (vec![y, uv], vec![y_stride, uv_stride], PixelFormat::P010Le)
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
fn download_hw_frame_into(
    dst: &mut ffmpeg::util::frame::video::Video,
    src: &ffmpeg::util::frame::video::Video,
) -> Result<()> {
    let ret = unsafe {
        ffmpeg::ffi::av_hwframe_transfer_data(dst.as_mut_ptr(), src.as_ptr(), 0)
    };
    if ret < 0 {
        anyhow::bail!("av_hwframe_transfer_data a échoué (code {ret})");
    }
    Ok(())
}
