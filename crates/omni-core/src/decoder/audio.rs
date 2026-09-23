use anyhow::{Context as _, Result};
use ffmpeg_next as ffmpeg;
use ffmpeg::software::resampling::context::Context as SwrContext;

/// Frame audio décodée — échantillons f32 packed interleaved, canaux natifs.
#[derive(Clone)]
pub struct DecodedAudioFrame {
    pub pts_secs:    f64,
    pub samples:     Vec<f32>,   // interleaved, `channels` canaux
    pub sample_rate: u32,
    pub channels:    u8,
}

/// Décodeur audio.
/// Sort en f32 packed, taux natif de la source, canaux natifs (1/2/6/8…).
/// Le moteur audio (omni-audio) fait le downmix + resampling vers le périphérique.
pub struct AudioDecoder {
    decoder:     ffmpeg::codec::decoder::Audio,
    resampler:   Option<SwrContext>,
    time_base:   f64,
    src_rate:    u32,
    src_layout:  ffmpeg::channel_layout::ChannelLayout,
    src_channels: u8,
    /// Vitesse de lecture demandée. À 1,0 le son sort tel quel ; sinon il
    /// passe par `atempo`, qui change la durée SANS changer la hauteur — ce
    /// que font VLC et mpv. Sans ça, régler la vitesse n'accélère que l'image
    /// et le son part en décalage.
    speed:       f32,
    /// Décalage du début du conteneur, retranché des horodatages.
    start_offset: f64,
    /// Format d'échantillon de la dernière frame décodée : sert à détecter un
    /// changement de paramètres en cours de flux.
    resampler_src_fmt: Option<ffmpeg::format::Sample>,
    /// Horodatage attendu de la prochaine image, reconstruit en comptant les
    /// échantillons. Sert quand le conteneur n'en fournit pas.
    next_pts: Option<f64>,
    /// Graphe `abuffer → atempo… → abuffersink`, reconstruit à chaque
    /// changement de vitesse.
    tempo:       Option<ffmpeg::filter::Graph>,
    /// Ancrages PTS pour retrouver le temps MÉDIA à partir du temps de sortie
    /// du filtre (qui est compressé par la vitesse) : (média, sortie).
    tempo_anchor: Option<(f64, f64)>,
    /// Frames produites par `atempo` en attente de livraison : un frame
    /// d'entrée peut en produire zéro, un ou plusieurs.
    pending:     std::collections::VecDeque<DecodedAudioFrame>,
}

impl AudioDecoder {
    pub fn new(decoder: ffmpeg::codec::decoder::Audio, time_base: f64) -> Result<Self> {
        let src_rate    = decoder.rate();
        let src_layout  = decoder.channel_layout();
        let src_channels = decoder.channels() as u8;
        Ok(Self {
            decoder, resampler: None, time_base, src_rate, src_layout, src_channels,
            speed: 1.0, start_offset: 0.0, resampler_src_fmt: None, next_pts: None,
            tempo: None, tempo_anchor: None,
            pending: std::collections::VecDeque::new(),
        })
    }

    pub fn send_packet(&mut self, packet: &ffmpeg::Packet) -> Result<()> {
        self.decoder.send_packet(packet).context("send_packet audio")
    }

    pub fn send_eof(&mut self) -> Result<()> {
        self.decoder.send_eof().context("send_eof audio")
    }

    pub fn receive_frame(&mut self) -> Result<Option<DecodedAudioFrame>> {
        if let Some(frame) = self.pending.pop_front() { return Ok(Some(frame)); }

        let mut raw = ffmpeg::util::frame::audio::Audio::empty();
        match self.decoder.receive_frame(&mut raw) {
            Ok(()) => {}
            Err(ffmpeg::Error::Other { errno: ffmpeg::error::EAGAIN }) => return Ok(None),
            Err(e) => return Err(e).context("receive_frame audio"),
        }

        // Horodatage : celui du conteneur quand il existe VRAIMENT. Certains
        // flux (WMA en ASF) donnent 0 à chaque image ; s'y fier fige l'horloge
        // de lecture à zéro et gèle tout. On poursuit alors le compte des
        // échantillons déjà joués.
        let container_pts = raw.pts()
            .filter(|p| *p != ffmpeg::ffi::AV_NOPTS_VALUE)
            .map(|p| (p as f64 * self.time_base - self.start_offset).max(0.0));
        let pts_secs = match (container_pts, self.next_pts) {
            (Some(p), Some(expected)) if p <= 0.0 && expected > 0.0 => expected,
            (Some(p), _) => p,
            (None, Some(expected)) => expected,
            (None, None) => 0.0,
        };

        // Conversion vers f32 entrelacé, faite à la main.
        //
        // Passer par `swr` obligeait à lui décrire la disposition des canaux à
        // l'avance ; or plusieurs codecs (WMA en tête) ne la révèlent qu'après
        // la première image, et FFmpeg 8 refuse alors la frame avec « Input
        // changed » — la piste se taisait entièrement. Les formats produits par
        // les décodeurs sont peu nombreux et la conversion tient en quelques
        // lignes, sans rien à déclarer d'avance.
        let frame_rate     = raw.rate();
        let frame_channels = raw.channels().max(1) as usize;
        let samples = match interleave_to_f32(&raw, frame_channels) {
            Some(v) => v,
            None => {
                anyhow::bail!("format audio non géré : {:?}", raw.format());
            }
        };
        self.src_rate     = frame_rate;
        self.src_channels = frame_channels as u8;
        if frame_rate > 0 {
            let dur = samples.len() as f64 / (frame_rate as f64 * frame_channels as f64);
            self.next_pts = Some(pts_secs + dur);
        }

        if (self.speed - 1.0).abs() >= 0.001 {
            let produced = self.stretch_samples(&samples, pts_secs)?;
            self.pending.extend(produced);
            return Ok(self.pending.pop_front());
        }

        Ok(Some(DecodedAudioFrame {
            pts_secs,
            samples,
            sample_rate: self.src_rate,
            channels:    self.src_channels,
        }))
    }

    /// Change la vitesse de lecture. Le graphe est reconstruit au prochain
    /// frame ; les frames déjà dans le filtre sont abandonnées (quelques
    /// dizaines de millisecondes, inaudible au moment d'un changement).
    pub fn set_start_offset(&mut self, secs: f64) { self.start_offset = secs; }

    pub fn set_speed(&mut self, speed: f32) {
        let speed = speed.clamp(0.25, 4.0);
        if (speed - self.speed).abs() < 0.001 { return; }
        self.speed = speed;
        self.tempo = None;
        self.tempo_anchor = None;
        self.pending.clear();
    }

    /// `atempo` n'accepte qu'un facteur entre 0,5 et 2 : au-delà on en
    /// enchaîne plusieurs, exactement comme le fait la ligne de commande
    /// FFmpeg.
    fn atempo_chain(speed: f32) -> String {
        let mut factors = Vec::new();
        let mut remaining = speed;
        while remaining > 2.0 { factors.push(2.0); remaining /= 2.0; }
        while remaining < 0.5 { factors.push(0.5); remaining /= 0.5; }
        factors.push(remaining);
        factors.iter()
            .map(|f| format!("atempo={f:.6}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn build_tempo(&mut self) -> Result<()> {
        let mut graph = ffmpeg::filter::Graph::new();
        let args = format!(
            "time_base=1/{rate}:sample_rate={rate}:sample_fmt=flt:channel_layout=0x{layout:x}",
            rate = self.src_rate,
            layout = self.src_layout.bits(),
        );
        graph.add(&ffmpeg::filter::find("abuffer").context("filtre abuffer absent")?, "in", &args)?;
        graph.add(&ffmpeg::filter::find("abuffersink").context("filtre abuffersink absent")?, "out", "")?;
        graph.output("in", 0)?.input("out", 0)?.parse(&Self::atempo_chain(self.speed))?;
        graph.validate()?;
        self.tempo = Some(graph);
        self.tempo_anchor = None;
        Ok(())
    }

    /// Passe des échantillons f32 entrelacés dans `atempo` et rend les frames
    /// produites, horodatées en temps MÉDIA.
    fn stretch_samples(&mut self, samples: &[f32], pts_secs: f64)
        -> Result<Vec<DecodedAudioFrame>>
    {
        if self.tempo.is_none() { self.build_tempo()?; }
        let rate = self.src_rate as f64;
        let channels = self.src_channels.max(1) as usize;
        let frames = samples.len() / channels;

        let mut input = ffmpeg::util::frame::audio::Audio::new(
            ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
            frames,
            self.src_layout,
        );
        unsafe {
            let dst = input.data_mut(0).as_mut_ptr() as *mut f32;
            std::ptr::copy_nonoverlapping(samples.as_ptr(), dst, samples.len());
        }
        let graph = self.tempo.as_mut().context("graphe atempo absent")?;
        input.set_pts(Some((pts_secs * rate) as i64));
        graph.get("in").context("entrée du graphe absente")?.source().add(&input)?;

        let mut out = Vec::new();
        loop {
            let mut filtered = ffmpeg::util::frame::audio::Audio::empty();
            let mut sink = graph.get("out").context("sortie du graphe absente")?;
            if sink.sink().frame(&mut filtered).is_err() { break; }
            let out_secs = filtered.pts().unwrap_or(0) as f64 / rate;
            // Le filtre comprime le temps : on rétablit le temps média pour que
            // l'horloge de lecture et la vidéo restent alignées.
            let (anchor_media, anchor_out) = *self.tempo_anchor
                .get_or_insert((pts_secs, out_secs));
            let media_secs = anchor_media + (out_secs - anchor_out) * self.speed as f64;
            out.push(DecodedAudioFrame {
                pts_secs:    media_secs,
                samples:     audio_frame_to_f32(&filtered),
                sample_rate: self.src_rate,
                channels:    self.src_channels,
            });
        }
        Ok(out)
    }

    pub fn sample_rate(&self) -> u32 { self.src_rate }
    pub fn channels(&self)    -> u8  { self.src_channels }
}

fn audio_frame_to_f32(frame: &ffmpeg::util::frame::audio::Audio) -> Vec<f32> {
    let data = frame.data(0);
    let n = data.len() / std::mem::size_of::<f32>();
    let mut out = vec![0f32; n];
    for (i, chunk) in data.chunks_exact(4).enumerate() {
        out[i] = f32::from_le_bytes(chunk.try_into().unwrap());
    }
    out
}

/// Convertit une frame audio décodée en `f32` entrelacés.
///
/// Couvre les formats que produisent réellement les décodeurs FFmpeg, planaires
/// comme entrelacés. Renvoie `None` pour un format inconnu, que l'appelant
/// signale plutôt que de jouer du bruit.
fn interleave_to_f32(
    frame: &ffmpeg::util::frame::audio::Audio,
    channels: usize,
) -> Option<Vec<f32>> {
    use ffmpeg::format::sample::Type::{Packed, Planar};
    use ffmpeg::format::Sample::*;

    let samples_per_ch = frame.samples();
    let mut out = vec![0f32; samples_per_ch * channels];

    // Lit l'échantillon `i` du plan `p`, converti en f32 dans [-1, 1].
    macro_rules! fill {
        ($t:ty, $conv:expr, $planar:expr) => {{
            let scale = $conv;
            if $planar {
                for ch in 0..channels {
                    let data = frame.data(ch);
                    let vals = unsafe {
                        std::slice::from_raw_parts(data.as_ptr() as *const $t, samples_per_ch)
                    };
                    for (i, v) in vals.iter().enumerate() {
                        out[i * channels + ch] = scale(*v);
                    }
                }
            } else {
                let data = frame.data(0);
                let vals = unsafe {
                    std::slice::from_raw_parts(
                        data.as_ptr() as *const $t,
                        samples_per_ch * channels,
                    )
                };
                for (i, v) in vals.iter().enumerate() {
                    out[i] = scale(*v);
                }
            }
        }};
    }

    match frame.format() {
        F32(Planar) => fill!(f32, |v: f32| v, true),
        F32(Packed) => fill!(f32, |v: f32| v, false),
        F64(Planar) => fill!(f64, |v: f64| v as f32, true),
        F64(Packed) => fill!(f64, |v: f64| v as f32, false),
        I16(Planar) => fill!(i16, |v: i16| v as f32 / 32768.0, true),
        I16(Packed) => fill!(i16, |v: i16| v as f32 / 32768.0, false),
        I32(Planar) => fill!(i32, |v: i32| v as f32 / 2_147_483_648.0, true),
        I32(Packed) => fill!(i32, |v: i32| v as f32 / 2_147_483_648.0, false),
        U8(Planar)  => fill!(u8, |v: u8| (v as f32 - 128.0) / 128.0, true),
        U8(Packed)  => fill!(u8, |v: u8| (v as f32 - 128.0) / 128.0, false),
        // `None` seul désignerait ici `Sample::None`, importé juste au-dessus.
        _ => return Option::None,
    }
    Some(out)
}
