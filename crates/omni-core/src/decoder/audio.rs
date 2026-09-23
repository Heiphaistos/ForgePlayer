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
            speed: 1.0, tempo: None, tempo_anchor: None,
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

        let pts_secs = raw.pts()
            .map(|p| p as f64 * self.time_base)
            .unwrap_or(0.0);

        // Conversion vers f32 packed, même taux, même layout
        let resampler = match &mut self.resampler {
            Some(r) => r,
            None => {
                let r = SwrContext::get(
                    raw.format(),
                    self.src_layout,
                    self.src_rate,
                    ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
                    self.src_layout,   // même layout — le downmix est dans AudioEngine
                    self.src_rate,
                )
                .context("création SwrContext")?;
                self.resampler = Some(r);
                self.resampler.as_mut().unwrap()
            }
        };

        let mut resampled = ffmpeg::util::frame::audio::Audio::empty();
        resampler.run(&raw, &mut resampled).context("resampling audio")?;

        if (self.speed - 1.0).abs() >= 0.001 {
            let produced = self.stretch(&resampled, pts_secs)?;
            self.pending.extend(produced);
            return Ok(self.pending.pop_front());
        }

        Ok(Some(DecodedAudioFrame {
            pts_secs,
            samples: audio_frame_to_f32(&resampled),
            sample_rate: self.src_rate,
            channels:    self.src_channels,
        }))
    }

    /// Change la vitesse de lecture. Le graphe est reconstruit au prochain
    /// frame ; les frames déjà dans le filtre sont abandonnées (quelques
    /// dizaines de millisecondes, inaudible au moment d'un changement).
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

    /// Passe un frame f32 packed dans `atempo` et rend les frames produites,
    /// horodatées en temps MÉDIA.
    fn stretch(&mut self, frame: &ffmpeg::util::frame::audio::Audio, pts_secs: f64)
        -> Result<Vec<DecodedAudioFrame>>
    {
        if self.tempo.is_none() { self.build_tempo()?; }
        let rate = self.src_rate as f64;
        let graph = self.tempo.as_mut().context("graphe atempo absent")?;
        let mut input = frame.clone();
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
