use anyhow::{Context as _, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{unbounded, Receiver, Sender};
use omni_core::decoder::DecodedAudioFrame;
use ringbuf::{HeapConsumer, HeapProducer, HeapRb};
use std::sync::{
    atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering},
    Arc,
};

use crate::resampler::AudioResampler;

const RING_SECS: usize = 8;  // 8 s de buffer pour absorber les rafales

pub struct AudioEngine {
    _stream:     cpal::Stream,
    sender:      Sender<(u64, DecodedAudioFrame)>,
    volume:      Arc<AtomicU32>,
    paused:      Arc<AtomicBool>,
    device_rate: u32,
    channels:    usize,
    ring_level:  Arc<AtomicU64>,
    generation:  Arc<AtomicU64>,
    consumer:    Arc<parking_lot::Mutex<HeapConsumer<f32>>>,
    /// PTS (secondes, f64 bits) de la fin du dernier frame poussé dans le ring.
    /// u64::MAX = sentinelle « pas encore de donnée » (après flush/démarrage).
    last_pts:    Arc<AtomicU64>,
}

const PTS_NONE: u64 = u64::MAX;

impl AudioEngine {
    pub fn new() -> Result<Self> {
        let host   = cpal::default_host();
        let device = host
            .default_output_device()
            .context("aucun périphérique audio")?;
        let config = device
            .default_output_config()
            .context("config audio par défaut")?;

        log::info!("audio device: {:?} — {:?}", device.name(), config);

        let device_rate = config.sample_rate().0;
        // Le pipeline downmixe TOUJOURS vers stéréo (fill_ring::downmix_to_stereo) —
        // le stream CPAL doit donc être ouvert en 2 canaux, jamais le nombre natif
        // du device. Sur un PC dont le device par défaut est en 5.1/7.1 (courant sur
        // desktop même sans enceintes surround), ignorer ça faisait lire le ring
        // (rempli en 2 canaux) comme des trames à N canaux : le ring se vidait N/2×
        // trop vite → lecture accélérée en continu, indépendante de toute action
        // utilisateur (pause/seek ne changeaient rien).
        const OUTPUT_CHANNELS: usize = 2;
        let channels    = OUTPUT_CHANNELS;
        let capacity    = device_rate as usize * channels * RING_SECS;

        let rb: HeapRb<f32> = HeapRb::new(capacity);
        let (producer, consumer) = rb.split();

        let volume     = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let paused     = Arc::new(AtomicBool::new(false));
        let ring_level = Arc::new(AtomicU64::new(0));
        let generation = Arc::new(AtomicU64::new(0));
        let last_pts   = Arc::new(AtomicU64::new(PTS_NONE));

        // Canal non-borné : push_frame ne bloque ni ne droppe jamais de frame.
        // Chaque frame est taguée avec la génération courante — flush() incrémente
        // la génération, les frames périmées sont jetées par fill_ring.
        let (tx, rx) = unbounded::<(u64, DecodedAudioFrame)>();

        // Consumer partagé entre le callback CPAL et flush() (vidage du ring au seek)
        let consumer = Arc::new(parking_lot::Mutex::new(consumer));

        {
            let ring_level2 = ring_level.clone();
            let gen2        = generation.clone();
            let last_pts2   = last_pts.clone();
            std::thread::Builder::new()
                .name("audio-fill".into())
                .spawn(move || fill_ring(rx, producer, device_rate, channels, ring_level2, gen2, last_pts2))?;
        }

        let vol_w      = volume.clone();
        let paused_w   = paused.clone();
        let rl_w       = ring_level.clone();
        let fmt        = config.sample_format();
        let mut cfg: cpal::StreamConfig = config.into();
        // Force le stream lui-même en stéréo — voir commentaire OUTPUT_CHANNELS.
        cfg.channels = OUTPUT_CHANNELS as u16;

        let stream = build_stream(&device, &cfg, fmt, consumer.clone(), vol_w, paused_w, rl_w)?;
        stream.play().context("play stream audio")?;

        Ok(Self {
            _stream: stream,
            sender: tx,
            volume,
            paused,
            device_rate,
            channels,
            ring_level,
            generation,
            consumer,
            last_pts,
        })
    }

    pub fn sample_rate(&self) -> u32 { self.device_rate }

    /// Secondes d'audio dans le ring buffer (mis à jour côté CPAL — valeur fraîche).
    pub fn buffered_secs(&self) -> f64 {
        let samples = self.ring_level.load(Ordering::Acquire) as f64;
        samples / (self.device_rate as f64 * self.channels as f64)
    }

    /// Pousse un frame décodé — canal non-borné, jamais de drop.
    pub fn push_frame(&self, frame: DecodedAudioFrame) {
        let _ = self.sender.send((self.generation.load(Ordering::Relaxed), frame));
    }

    /// Vide tout l'audio en attente (ring + canal) — à appeler au seek et au
    /// changement de fichier pour éviter de jouer l'ancien flux (désync A/V).
    pub fn flush(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.last_pts.store(PTS_NONE, Ordering::Relaxed);
        let mut c = self.consumer.lock();
        let n = c.len();
        c.skip(n);
        drop(c);
        self.ring_level.store(0, Ordering::Release);
    }

    /// Position de lecture réelle estimée (secondes de média) : PTS de fin du
    /// dernier frame entré dans le ring, moins ce qui reste à jouer dans le ring.
    /// None tant qu'aucune donnée n'est arrivée (démarrage, juste après flush).
    pub fn playback_position(&self) -> Option<f64> {
        let bits = self.last_pts.load(Ordering::Relaxed);
        if bits == PTS_NONE { return None; }
        let last = f64::from_bits(bits);
        Some((last - self.buffered_secs()).max(0.0))
    }

    pub fn set_volume(&self, v: f32) {
        self.volume.store(v.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_paused(&self, p: bool) {
        self.paused.store(p, Ordering::Relaxed);
    }
}

// ─── Construction du stream CPAL ────────────────────────────────────────────

fn build_stream(
    device:     &cpal::Device,
    cfg:        &cpal::StreamConfig,
    fmt:        cpal::SampleFormat,
    cons:       Arc<parking_lot::Mutex<HeapConsumer<f32>>>,
    volume:     Arc<AtomicU32>,
    paused:     Arc<AtomicBool>,
    ring_level: Arc<AtomicU64>,
) -> Result<cpal::Stream> {
    let err_fn = |e: cpal::StreamError| log::error!("cpal error: {e}");

    let stream = match fmt {
        cpal::SampleFormat::F32 => {
            let cons_w = cons.clone();
            let vol_w  = volume.clone();
            let pau_w  = paused.clone();
            let rl_w   = ring_level.clone();
            device.build_output_stream(cfg, move |data: &mut [f32], _| {
                if pau_w.load(Ordering::Relaxed) { data.fill(0.0); return; }
                let mut c = cons_w.lock();
                let n = c.pop_slice(data);
                let remaining = c.len();
                drop(c);
                rl_w.store(remaining as u64, Ordering::Release);
                if n < data.len() { data[n..].fill(0.0); }
                let vol = f32::from_bits(vol_w.load(Ordering::Relaxed));
                if (vol - 1.0).abs() > 0.001 {
                    for s in data.iter_mut() { *s *= vol; }
                }
            }, err_fn, None)?
        }
        cpal::SampleFormat::I16 => {
            let cons_w = cons.clone();
            let vol_w  = volume.clone();
            let pau_w  = paused.clone();
            let rl_w   = ring_level.clone();
            // Pre-allocated scratch: grows to max callback size, never reallocates after warm-up.
            let mut scratch = Vec::<f32>::new();
            device.build_output_stream(cfg, move |data: &mut [i16], _| {
                if pau_w.load(Ordering::Relaxed) { data.fill(0); return; }
                scratch.resize(data.len(), 0.0);
                let mut c = cons_w.lock();
                let n = c.pop_slice(&mut scratch);
                let remaining = c.len();
                drop(c);
                rl_w.store(remaining as u64, Ordering::Release);
                if n < scratch.len() { scratch[n..].fill(0.0); }
                let vol = f32::from_bits(vol_w.load(Ordering::Relaxed));
                for (o, s) in data.iter_mut().zip(scratch.iter()) {
                    *o = ((s * vol).clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                }
            }, err_fn, None)?
        }
        cpal::SampleFormat::U16 => {
            let cons_w = cons.clone();
            let vol_w  = volume.clone();
            let pau_w  = paused.clone();
            let rl_w   = ring_level.clone();
            // Pre-allocated scratch: grows to max callback size, never reallocates after warm-up.
            let mut scratch = Vec::<f32>::new();
            device.build_output_stream(cfg, move |data: &mut [u16], _| {
                if pau_w.load(Ordering::Relaxed) { data.fill(32768); return; }
                scratch.resize(data.len(), 0.0);
                let mut c = cons_w.lock();
                let n = c.pop_slice(&mut scratch);
                let remaining = c.len();
                drop(c);
                rl_w.store(remaining as u64, Ordering::Release);
                if n < scratch.len() { scratch[n..].fill(0.0); }
                let vol = f32::from_bits(vol_w.load(Ordering::Relaxed));
                for (o, s) in data.iter_mut().zip(scratch.iter()) {
                    *o = (((s * vol).clamp(-1.0, 1.0) + 1.0) * 0.5 * u16::MAX as f32) as u16;
                }
            }, err_fn, None)?
        }
        cpal::SampleFormat::I32 => {
            let cons_w = cons.clone();
            let vol_w  = volume.clone();
            let pau_w  = paused.clone();
            let rl_w   = ring_level.clone();
            let mut scratch = Vec::<f32>::new();
            device.build_output_stream(cfg, move |data: &mut [i32], _| {
                if pau_w.load(Ordering::Relaxed) { data.fill(0); return; }
                scratch.resize(data.len(), 0.0);
                let mut c = cons_w.lock();
                let n = c.pop_slice(&mut scratch);
                let remaining = c.len();
                drop(c);
                rl_w.store(remaining as u64, Ordering::Release);
                if n < scratch.len() { scratch[n..].fill(0.0); }
                let vol = f32::from_bits(vol_w.load(Ordering::Relaxed));
                for (o, s) in data.iter_mut().zip(scratch.iter()) {
                    *o = ((*s * vol).clamp(-1.0, 1.0) * i32::MAX as f32) as i32;
                }
            }, err_fn, None)?
        }
        cpal::SampleFormat::F64 => {
            let cons_w = cons.clone();
            let vol_w  = volume.clone();
            let pau_w  = paused.clone();
            let rl_w   = ring_level.clone();
            let mut scratch = Vec::<f32>::new();
            device.build_output_stream(cfg, move |data: &mut [f64], _| {
                if pau_w.load(Ordering::Relaxed) { data.fill(0.0); return; }
                scratch.resize(data.len(), 0.0);
                let mut c = cons_w.lock();
                let n = c.pop_slice(&mut scratch);
                let remaining = c.len();
                drop(c);
                rl_w.store(remaining as u64, Ordering::Release);
                if n < scratch.len() { scratch[n..].fill(0.0); }
                let vol = f32::from_bits(vol_w.load(Ordering::Relaxed));
                for (o, s) in data.iter_mut().zip(scratch.iter()) {
                    *o = ((*s * vol).clamp(-1.0, 1.0)) as f64;
                }
            }, err_fn, None)?
        }
        cpal::SampleFormat::U32 => {
            let cons_w = cons.clone();
            let vol_w  = volume.clone();
            let pau_w  = paused.clone();
            let rl_w   = ring_level.clone();
            let mut scratch = Vec::<f32>::new();
            device.build_output_stream(cfg, move |data: &mut [u32], _| {
                if pau_w.load(Ordering::Relaxed) { data.fill(u32::MAX / 2); return; }
                scratch.resize(data.len(), 0.0);
                let mut c = cons_w.lock();
                let n = c.pop_slice(&mut scratch);
                let remaining = c.len();
                drop(c);
                rl_w.store(remaining as u64, Ordering::Release);
                if n < scratch.len() { scratch[n..].fill(0.0); }
                let vol = f32::from_bits(vol_w.load(Ordering::Relaxed));
                for (o, s) in data.iter_mut().zip(scratch.iter()) {
                    *o = (((*s * vol).clamp(-1.0, 1.0) + 1.0) * 0.5 * u32::MAX as f32) as u32;
                }
            }, err_fn, None)?
        }
        cpal::SampleFormat::I8 => {
            let cons_w = cons.clone();
            let vol_w  = volume.clone();
            let pau_w  = paused.clone();
            let rl_w   = ring_level.clone();
            let mut scratch = Vec::<f32>::new();
            device.build_output_stream(cfg, move |data: &mut [i8], _| {
                if pau_w.load(Ordering::Relaxed) { data.fill(0); return; }
                scratch.resize(data.len(), 0.0);
                let mut c = cons_w.lock();
                let n = c.pop_slice(&mut scratch);
                let remaining = c.len();
                drop(c);
                rl_w.store(remaining as u64, Ordering::Release);
                if n < scratch.len() { scratch[n..].fill(0.0); }
                let vol = f32::from_bits(vol_w.load(Ordering::Relaxed));
                for (o, s) in data.iter_mut().zip(scratch.iter()) {
                    *o = ((*s * vol).clamp(-1.0, 1.0) * i8::MAX as f32) as i8;
                }
            }, err_fn, None)?
        }
        cpal::SampleFormat::U8 => {
            let cons_w = cons.clone();
            let vol_w  = volume.clone();
            let pau_w  = paused.clone();
            let rl_w   = ring_level.clone();
            let mut scratch = Vec::<f32>::new();
            device.build_output_stream(cfg, move |data: &mut [u8], _| {
                if pau_w.load(Ordering::Relaxed) { data.fill(128); return; }
                scratch.resize(data.len(), 0.0);
                let mut c = cons_w.lock();
                let n = c.pop_slice(&mut scratch);
                let remaining = c.len();
                drop(c);
                rl_w.store(remaining as u64, Ordering::Release);
                if n < scratch.len() { scratch[n..].fill(0.0); }
                let vol = f32::from_bits(vol_w.load(Ordering::Relaxed));
                for (o, s) in data.iter_mut().zip(scratch.iter()) {
                    *o = (((*s * vol).clamp(-1.0, 1.0) + 1.0) * 0.5 * u8::MAX as f32) as u8;
                }
            }, err_fn, None)?
        }
        fmt => anyhow::bail!("format CPAL non géré: {fmt:?}"),
    };

    Ok(stream)
}

// ─── Thread fill_ring ────────────────────────────────────────────────────────

fn fill_ring(
    rx:           Receiver<(u64, DecodedAudioFrame)>,
    mut producer: HeapProducer<f32>,
    device_rate:  u32,
    dev_ch:       usize,
    ring_level:   Arc<AtomicU64>,
    generation:   Arc<AtomicU64>,
    last_pts:     Arc<AtomicU64>,
) {
    let mut resampler: Option<AudioResampler> = None;

    for (gen, frame) in rx {
        // Frame d'une génération périmée (flush depuis) → jeter
        if gen != generation.load(Ordering::Relaxed) { continue; }

        // PTS de fin du frame — sert d'horloge de lecture (playback_position)
        let frame_dur = if frame.sample_rate > 0 && frame.channels > 0 {
            frame.samples.len() as f64
                / (frame.sample_rate as f64 * frame.channels as f64)
        } else { 0.0 };
        last_pts.store((frame.pts_secs + frame_dur).to_bits(), Ordering::Relaxed);
        let in_ch   = frame.channels as usize;
        let in_rate = frame.sample_rate;

        let stereo = if in_ch == dev_ch {
            frame.samples.clone()
        } else {
            downmix_to_stereo(&frame.samples, in_ch)
        };

        // Si le resampler manque ou échoue, ne JAMAIS jouer les échantillons bruts :
        // playing at the wrong rate is audible as permanent pitch/speed-up
        // ("chipmunk"), and looks unfixable to the user (pause/seek don't help since
        // it's a per-frame code path, not a transient state). On saute le frame et on
        // réessaie la construction au frame suivant plutôt que de jouer du son corrompu.
        let final_samples: Option<Vec<f32>> = if in_rate == device_rate {
            Some(stereo)
        } else {
            let out_ch = dev_ch.min(2);
            let needs_new = resampler.as_ref()
                .map(|r| r.in_rate() != in_rate || r.out_rate() != device_rate)
                .unwrap_or(true);
            if needs_new {
                match AudioResampler::new(in_rate, device_rate, out_ch) {
                    Ok(r) => resampler = Some(r),
                    Err(e) => {
                        resampler = None;
                        log::error!("resampler {in_rate}Hz→{device_rate}Hz: {e:#} — frame audio ignoré (pas de lecture à mauvaise vitesse)");
                    }
                }
            }
            match resampler.as_mut() {
                Some(r) => match r.process_interleaved(&stereo) {
                    Ok(out) => Some(out),
                    Err(e) => {
                        log::error!("resample: {e:#} — frame audio ignoré");
                        None
                    }
                },
                None => None,
            }
        };
        let Some(final_samples) = final_samples else { continue };

        let pushed = producer.push_slice(&final_samples);
        // ring_level mis à jour ici aussi pour le fill ; CPAL met à jour côté consommation
        ring_level.store(producer.len() as u64, Ordering::Release);

        if pushed < final_samples.len() {
            log::warn!("audio ring overflow: dropped {} samples", final_samples.len() - pushed);
        }
    }
}

// ─── Downmix N canaux → stéréo ───────────────────────────────────────────────

/// Coefficients de mixage vers la stéréo, dans l'ordre des canaux de FFmpeg.
///
/// 5.1 = FL FR FC LFE BL BR, 7.1 = FL FR FC LFE BL BR SL SR. Le caisson (LFE)
/// n'est pas mélangé, comme le fait FFmpeg par défaut : son contenu est déjà
/// présent dans les autres canaux et l'ajouter ne fait que saturer.
const COEFF_51: [(f32, f32); 6] = [
    (1.0, 0.0), (0.0, 1.0), (0.707, 0.707), (0.0, 0.0), (0.707, 0.0), (0.0, 0.707),
];
const COEFF_71: [(f32, f32); 8] = [
    (1.0, 0.0), (0.0, 1.0), (0.707, 0.707), (0.0, 0.0),
    (0.5, 0.0), (0.0, 0.5), (0.707, 0.0), (0.0, 0.707),
];

/// Replie un frame multicanal en stéréo.
///
/// La somme est NORMALISÉE par le total des coefficients : sans ça, un passage
/// fort en 5.1 dépasse largement 1,0 et se faisait simplement écrêter, ce qui
/// s'entend comme de la distorsion sur tous les films multicanaux. C'est la
/// même normalisation que celle de FFmpeg (`rematrix` avec `normalize`) et de
/// VLC : le repli est plus discret qu'une piste stéréo d'origine, mais il ne
/// sature jamais.
fn downmix_to_stereo(samples: &[f32], in_ch: usize) -> Vec<f32> {
    if in_ch == 0 { return Vec::new(); }
    let frames = samples.len() / in_ch;
    let mut out = Vec::with_capacity(frames * 2);

    let coeffs: Option<&[(f32, f32)]> = match in_ch {
        6 => Some(&COEFF_51),
        8 => Some(&COEFF_71),
        _ => None,
    };
    // Facteur de normalisation : le canal le plus chargé ne peut plus dépasser 1.
    let norm = coeffs.map(|c| {
        let l: f32 = c.iter().map(|(a, _)| *a).sum();
        let r: f32 = c.iter().map(|(_, b)| *b).sum();
        1.0 / l.max(r).max(1.0)
    }).unwrap_or(1.0);

    for f in 0..frames {
        let b = f * in_ch;
        let (l, r) = match (in_ch, coeffs) {
            (1, _) => { let m = samples[b]; (m, m) }
            (2, _) => (samples[b], samples[b + 1]),
            (_, Some(c)) => {
                let mut l = 0.0;
                let mut r = 0.0;
                for (ch, (cl, cr)) in c.iter().enumerate() {
                    l += samples[b + ch] * cl;
                    r += samples[b + ch] * cr;
                }
                (l * norm, r * norm)
            }
            (n, None) => {
                // Disposition inconnue : canaux pairs à gauche, impairs à droite.
                let (mut ls, mut rs) = (0f32, 0f32);
                for ch in 0..n {
                    if ch % 2 == 0 { ls += samples[b + ch]; }
                    else           { rs += samples[b + ch]; }
                }
                let h = (n / 2).max(1) as f32;
                (ls / h, rs / h)
            }
        };
        // Filet de sécurité : une source déjà saturée ne doit pas dépasser.
        out.push(l.clamp(-1.0, 1.0));
        out.push(r.clamp(-1.0, 1.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::downmix_to_stereo;

    #[test]
    fn mono_est_duplique() {
        assert_eq!(downmix_to_stereo(&[0.5, -0.25], 1), vec![0.5, 0.5, -0.25, -0.25]);
    }

    #[test]
    fn stereo_passe_tel_quel() {
        assert_eq!(downmix_to_stereo(&[0.5, -0.5], 2), vec![0.5, -0.5]);
    }

    #[test]
    fn cinq_un_ne_sature_jamais() {
        // Tous les canaux à fond : avant normalisation la somme valait 2,41.
        let out = downmix_to_stereo(&[1.0; 6], 6);
        assert_eq!(out.len(), 2);
        for v in out {
            assert!(v <= 1.0, "sortie {v} au-dessus de 1,0 : écrêtage");
            assert!((v - 1.0).abs() < 1e-6, "sortie {v} : la pleine échelle doit donner 1,0");
        }
    }

    #[test]
    fn cinq_un_respecte_les_canaux() {
        // Avant gauche seul → rien à droite.
        let mut input = [0.0f32; 6];
        input[0] = 1.0;
        let out = downmix_to_stereo(&input, 6);
        assert!(out[0] > 0.4 && out[0] < 0.42, "gauche = {}", out[0]);
        assert_eq!(out[1], 0.0, "le canal droit doit rester muet");

        // Le caisson (LFE) n'est pas mélangé.
        let mut lfe = [0.0f32; 6];
        lfe[3] = 1.0;
        assert_eq!(downmix_to_stereo(&lfe, 6), vec![0.0, 0.0]);
    }

    #[test]
    fn sept_un_ne_sature_jamais() {
        let out = downmix_to_stereo(&[1.0; 8], 8);
        for v in out {
            assert!(v <= 1.0 + 1e-6, "sortie {v} au-dessus de 1,0");
            assert!((v - 1.0).abs() < 1e-6, "sortie {v} : la pleine échelle doit donner 1,0");
        }
    }

    #[test]
    fn longueur_de_sortie() {
        let frames = 5;
        let out = downmix_to_stereo(&vec![0.1; frames * 6], 6);
        assert_eq!(out.len(), frames * 2);
    }
}

