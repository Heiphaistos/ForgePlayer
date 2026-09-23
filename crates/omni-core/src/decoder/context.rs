use anyhow::{Context as _, Result};
use ffmpeg_next as ffmpeg;

use crate::hw_accel::HwAccelContext;

/// Contexte de décodage principal — ouvre le fichier et initialise les codecs.
pub struct DecodeContext {
    pub format_ctx: ffmpeg::format::context::Input,
    pub video_stream_idx: Option<usize>,
    pub audio_stream_idx: Option<usize>,
    pub subtitle_stream_idx: Option<usize>,
    pub hw_accel: Option<HwAccelContext>,
}

/// Vrai pour une source réseau (le reste est un chemin local).
pub(crate) fn is_network_url(path: &str) -> bool {
    const SCHEMES: [&str; 10] = [
        "http://", "https://", "rtsp://", "rtmp://", "rtmps://",
        "udp://", "rtp://", "srt://", "mms://", "mmsh://",
    ];
    let lower = path.to_ascii_lowercase();
    SCHEMES.iter().any(|s| lower.starts_with(s))
}

/// Options passées à libavformat pour une source réseau.
///
/// Sans elles, une URL injoignable bloque sur le délai par défaut de FFmpeg
/// (mesuré : 10 s d'écran « Chargement… » avant le message d'erreur) et la
/// moindre coupure réseau en cours de lecture met fin au flux définitivement.
/// Avec, l'échec est annoncé en ~5 s et une coupure passagère est rattrapée
/// automatiquement, comme le font VLC et mpv.
pub(crate) fn network_options() -> ffmpeg::Dictionary<'static> {
    let mut opts = ffmpeg::Dictionary::new();
    // Délais en microsecondes : `timeout` borne la connexion TCP elle-même,
    // `rw_timeout` les lectures/écritures bloquantes ensuite.
    // La sonde puis l'ouverture réelle tentent chacune la connexion : un
    // délai de 3 s borne l'échec total à ~6 s d'attente pour l'utilisateur.
    opts.set("timeout", "3000000");
    // Les lectures, elles, restent tolérantes : un flux en direct peut marquer
    // une pause légitime sans que la lecture doive s'arrêter.
    opts.set("rw_timeout", "8000000");
    // Reconnexion automatique après une coupure EN COURS de lecture.
    // ⚠ Ne PAS activer `reconnect_on_network_error` : il fait aussi réessayer
    // la connexion initiale en boucle, donc une URL injoignable n'échoue
    // jamais (mesuré : plus aucune erreur au bout de 16 s, contre 10 s sans
    // l'option).
    opts.set("reconnect", "1");
    opts.set("reconnect_streamed", "1");
    opts.set("reconnect_delay_max", "4");
    opts.set("user_agent", concat!("ForgePlayer/", env!("CARGO_PKG_VERSION")));
    opts
}

impl DecodeContext {
    /// Ouvre un fichier local ou une URL réseau (HTTP/RTSP/RTMP/HLS).
    pub fn open(path: &str, preferred_hw: Option<&str>) -> Result<Self> {
        Self::open_with_device(path, preferred_hw, std::ptr::null_mut())
    }

    /// Variante qui réutilise un appareil D3D11 existant (celui du rendu) pour
    /// permettre le partage des surfaces sans copie.
    pub fn open_with_device(
        path: &str,
        preferred_hw: Option<&str>,
        d3d11_device: *mut std::ffi::c_void,
    ) -> Result<Self> {
        ffmpeg::init().context("ffmpeg::init")?;

        let format_ctx = if is_network_url(path) {
            ffmpeg::format::input_with_dictionary(&path, network_options())
                .with_context(|| format!("impossible d'ouvrir '{path}'"))?
        } else {
            ffmpeg::format::input(&path)
                .with_context(|| format!("impossible d'ouvrir '{path}'"))?
        };

        let video_stream_idx = format_ctx
            .streams()
            .best(ffmpeg::media::Type::Video)
            .map(|s| s.index());

        let audio_stream_idx = format_ctx
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .map(|s| s.index());

        let subtitle_stream_idx = format_ctx
            .streams()
            .best(ffmpeg::media::Type::Subtitle)
            .map(|s| s.index());

        let hw_accel = if !d3d11_device.is_null() {
            #[cfg(windows)]
            {
                match HwAccelContext::from_existing_d3d11(d3d11_device) {
                    Ok(ctx) => Some(ctx),
                    Err(e) => {
                        log::warn!("appareil D3D11 partagé refusé ({e:#}) — appareil séparé");
                        preferred_hw.and_then(|name| HwAccelContext::try_init(name).ok())
                    }
                }
            }
            #[cfg(not(windows))]
            { preferred_hw.and_then(|name| HwAccelContext::try_init(name).ok()) }
        } else {
            preferred_hw.and_then(|name| HwAccelContext::try_init(name).ok())
        };

        Ok(Self {
            format_ctx,
            video_stream_idx,
            audio_stream_idx,
            subtitle_stream_idx,
            hw_accel,
        })
    }

    /// Construit un décodeur vidéo pour le flux sélectionné.
    pub fn build_video_decoder(&self) -> Result<ffmpeg::codec::decoder::Video> {
        let stream_idx = self
            .video_stream_idx
            .context("aucun flux vidéo trouvé")?;
        let stream = self
            .format_ctx
            .stream(stream_idx)
            .context("stream vidéo introuvable")?;

        let mut codec_ctx =
            ffmpeg::codec::context::Context::from_parameters(stream.parameters())
                .context("création du contexte codec vidéo")?;

        // Active l'accélération matérielle si disponible
        if let Some(hw) = &self.hw_accel {
            hw.apply_to_codec(&mut codec_ctx);
        }

        codec_ctx
            .decoder()
            .video()
            .context("ouverture décodeur vidéo")
    }

    /// Construit un décodeur audio pour le flux sélectionné.
    pub fn build_audio_decoder(&self) -> Result<ffmpeg::codec::decoder::Audio> {
        let stream_idx = self
            .audio_stream_idx
            .context("aucun flux audio trouvé")?;
        self.build_audio_decoder_for(stream_idx)
    }

    /// Construit un décodeur audio pour un flux arbitraire (changement de piste).
    pub fn build_audio_decoder_for(&self, stream_idx: usize) -> Result<ffmpeg::codec::decoder::Audio> {
        let stream = self
            .format_ctx
            .stream(stream_idx)
            .context("stream audio introuvable")?;

        ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .context("création du contexte codec audio")?
            .decoder()
            .audio()
            .context("ouverture décodeur audio")
    }

    /// Retourne la durée totale en secondes.
    pub fn duration_secs(&self) -> f64 {
        self.format_ctx.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
    }

    /// Seek vers une position en secondes.
    /// av_seek_frame + BACKWARD : se cale sur la keyframe ≤ cible (standard lecteurs).
    /// L'ancien `format_ctx.seek(ts, ts..)` (min_ts = cible) échouait en EPERM sur MP4.
    /// Décalage du début du flux, en secondes.
    ///
    /// Un MPEG-2 TS ne commence pas à zéro : FFmpeg y rapporte des horodatages
    /// qui démarrent vers 1,4 s. Sans retrancher ce décalage, la position
    /// affichée est en avance d'autant et un saut tombe à côté.
    pub fn start_offset_secs(&self) -> f64 {
        let start = unsafe { (*self.format_ctx.as_ptr()).start_time };
        if start == ffmpeg::ffi::AV_NOPTS_VALUE || start <= 0 {
            0.0
        } else {
            start as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
        }
    }

    pub fn seek(&mut self, position_secs: f64) -> Result<()> {
        // La cible est exprimée dans le temps vu par l'utilisateur (qui part de
        // zéro) : on repasse dans le temps du conteneur.
        let position_secs = position_secs + self.start_offset_secs();
        let ts = (position_secs * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
        unsafe {
            let ret = ffmpeg::ffi::av_seek_frame(
                self.format_ctx.as_mut_ptr(),
                -1,
                ts,
                ffmpeg::ffi::AVSEEK_FLAG_BACKWARD,
            );
            if ret >= 0 { return Ok(()); }
            // Fallback pour les formats sans av_seek_frame fonctionnel
            let ret2 = ffmpeg::ffi::avformat_seek_file(
                self.format_ctx.as_mut_ptr(),
                -1,
                i64::MIN,
                ts,
                i64::MAX,
                ffmpeg::ffi::AVSEEK_FLAG_BACKWARD,
            );
            if ret2 >= 0 { return Ok(()); }
            anyhow::bail!("seek échoué (av_seek_frame={ret}, avformat_seek_file={ret2})");
        }
    }
}
