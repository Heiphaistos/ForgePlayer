use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
use ffmpeg_next as ffmpeg;

use crate::decoder::{
    audio::AudioDecoder, video::VideoDecoder, DecodedAudioFrame, DecodedVideoFrame,
};
use crate::decoder::context::DecodeContext;
use crate::pipeline::video_worker::{self, VideoWorkerMsg};
use crate::pipeline::{PipelineCommand, PipelineEvent};
use crate::probe;

/// Profondeur de la queue de paquets vidéo COMPRESSÉS envoyés au thread de
/// décodage vidéo dédié. `try_send` (jamais bloquant) : si le décodage vidéo
/// prend du retard, on droppe le paquet plutôt que de bloquer ce thread — sinon
/// l'audio (lu/décodé ici, sur ce même thread) se retrouverait affamé derrière
/// un GOP 4K/HDR lent à décoder, exactement le bug qu'on corrige.
const VIDEO_PKT_QUEUE_DEPTH: usize = 64;

pub fn run_demuxer(
    path:          &str,
    hw_accel_pref: &str,
    cmd_rx:        Receiver<PipelineCommand>,
    event_tx:      Sender<PipelineEvent>,
    video_tx:      Sender<DecodedVideoFrame>,
    audio_tx:      Sender<DecodedAudioFrame>,
) -> Result<()> {
    let info = probe::probe_file(std::path::Path::new(path))
        .unwrap_or_else(|_| probe::MediaInfo {
            path: path.to_string(),
            duration_secs: 0.0,
            video: None,
            audio: vec![],
            subtitles: vec![],
            chapters: vec![],
            format_name: "unknown".into(),
            bit_rate: 0,
        });

    let duration = info.duration_secs;
    let _ = event_tx.send(PipelineEvent::MetadataReady(Box::new(info)));
    let _ = event_tx.send(PipelineEvent::DurationKnown(duration));

    // Réglage Paramètres : "none" désactive tout (échappatoire si un pilote GPU
    // pose problème), "d3d11va"/"dxva2" force un choix précis, "auto" (défaut)
    // préfère d3d11va (API moderne) si dispo sinon dxva2.
    let preferred_hw: Option<&str> = match hw_accel_pref {
        "none"    => None,
        "d3d11va" => Some("d3d11va"),
        "dxva2"   => Some("dxva2"),
        _ => {
            #[cfg(windows)]
            {
                use crate::hw_accel::windows::win::is_d3d11va_available;
                if is_d3d11va_available() { Some("d3d11va") } else { Some("dxva2") }
            }
            #[cfg(not(windows))]
            { None }
        }
    };

    let mut ctx = DecodeContext::open(path, preferred_hw)?;

    // Indexe tous les flux audio disponibles pour le changement de piste
    let all_audio_idx: Vec<usize> = ctx.format_ctx
        .streams()
        .filter(|s| s.parameters().medium() == ffmpeg::media::Type::Audio)
        .map(|s| s.index())
        .collect();

    // Indexe tous les flux subtitle disponibles pour le changement de piste
    let all_sub_idx: Vec<usize> = ctx.format_ctx
        .streams()
        .filter(|s| s.parameters().medium() == ffmpeg::media::Type::Subtitle)
        .map(|s| s.index())
        .collect();

    let v_idx = ctx.video_stream_idx;
    let mut a_idx = ctx.audio_stream_idx;

    let v_tb = v_idx.and_then(|i| {
        ctx.format_ctx.stream(i).map(|s| {
            s.time_base().numerator() as f64 / s.time_base().denominator().max(1) as f64
        })
    }).unwrap_or(0.0);
    let mut a_tb = a_idx.and_then(|i| {
        ctx.format_ctx.stream(i).map(|s| {
            s.time_base().numerator() as f64 / s.time_base().denominator().max(1) as f64
        })
    }).unwrap_or(0.0);

    let initial_video_dec = v_idx
        .map(|_| ctx.build_video_decoder().map(|d| VideoDecoder::new(d, v_tb)))
        .transpose()?
        .and_then(|r| r.ok());
    log::debug!(
        "DBGPROBE demuxer: v_idx={v_idx:?} a_idx={a_idx:?} video_dec_ok={} ",
        initial_video_dec.is_some()
    );

    // Décodage vidéo sur son propre thread — voir video_worker.rs. `preview_dec`
    // reste local à CE thread : la preview post-seek-en-pause décode directement
    // depuis ctx.format_ctx dans une boucle autonome bornée, indépendante du flux
    // principal de paquets envoyés au worker.
    let (video_pkt_tx, video_pkt_rx) = bounded::<VideoWorkerMsg>(VIDEO_PKT_QUEUE_DEPTH);
    let (eof_ack_tx, eof_ack_rx)     = bounded::<()>(1);
    let _video_worker = video_worker::spawn(
        initial_video_dec, video_pkt_rx, video_tx.clone(), event_tx.clone(), eof_ack_tx,
    )?;
    let mut preview_dec: Option<VideoDecoder> = None;

    // Une piste audio illisible (codec non supporté, paramètres corrompus) ne
    // doit jamais tuer toute la lecture — avant, `?` propageait cette erreur et
    // arrêtait AUSSI la vidéo, ce qui pouvait ressembler à "codec manquant" pour
    // un fichier par ailleurs parfaitement lisible. On dégrade en vidéo muette
    // + avis OSD non-bloquant au lieu d'un échec complet.
    let mut audio_dec = match a_idx {
        None => None,
        Some(_) => match ctx.build_audio_decoder().and_then(|d| AudioDecoder::new(d, a_tb)) {
            Ok(dec) => Some(dec),
            Err(e) => {
                log::warn!("piste audio illisible, lecture vidéo seule: {e:#}");
                let _ = event_tx.send(PipelineEvent::Warning(
                    "Piste audio illisible (codec non supporté) — lecture sans son.".to_string()
                ));
                None
            }
        },
    };

    // Décodeurs sous-titres : TOUTES les pistes texte dès le départ. Les paquets ne
    // sont lus qu'une seule fois (en avance sur la lecture) — une activation tardive
    // de la piste doit retrouver les cues déjà passés. Le player filtre par piste.
    // (stream_idx, ordinal piste, décodeur, time_base)
    let mut sub_decs: Vec<(usize, usize, ffmpeg::codec::decoder::Subtitle, f64)> = Vec::new();
    for (ord, &si) in all_sub_idx.iter().enumerate() {
        if let Some(st) = ctx.format_ctx.stream(si) {
            let tb = st.time_base().numerator() as f64
                / st.time_base().denominator().max(1) as f64;
            match ffmpeg::codec::context::Context::from_parameters(st.parameters())
                .ok()
                .and_then(|cc| cc.decoder().subtitle().ok())
            {
                Some(dec) => sub_decs.push((si, ord, dec, tb)),
                None => log::warn!("piste sous-titre {si}: codec non supporté (bitmap PGS/VOBSUB ?)"),
            }
        }
    }

    let mut paused = false;
    // Après un seek en pause : décoder une frame vidéo pour rafraîchir l'affichage
    let mut preview_after_seek = false;
    // Après seek : av_seek_frame se cale sur la keyframe ≤ cible. On décode depuis
    // la keyframe mais on jette les frames jusqu'au PTS cible (précision à la frame).
    let mut v_skip_until: Option<f64> = None;
    let mut a_skip_until: Option<f64> = None;

    // DBGPROBE (diagnostic temporaire, RUST_LOG=debug)
    let dbg_start = std::time::Instant::now();
    let mut dbg_v_pkts = 0u64;
    let mut dbg_v_dropped = 0u64;
    let mut dbg_a_frames = 0u64;

    'main: loop {
        // Traite toutes les commandes en attente
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                PipelineCommand::Stop   => break 'main,
                PipelineCommand::Pause  => paused = true,
                PipelineCommand::Resume => paused = false,
                PipelineCommand::Seek(pos) => {
                    // Un seek raté (flux réseau, format exotique) ne doit pas tuer la
                    // lecture : on log et on continue à la position courante.
                    if let Err(e) = ctx.seek(pos) {
                        log::warn!("seek ignoré: {e:#}");
                        continue;
                    }
                    // Vide les buffers internes du décodeur audio (local à ce thread)
                    if let Some(dec) = &mut audio_dec {
                        let _ = dec.send_eof();
                        while dec.receive_frame().ok().flatten().is_some() {}
                    }
                    // Reconstruit les décodeurs pour repartir d'un état propre. Le
                    // décodeur vidéo vit sur le thread worker : on lui envoie le nouveau,
                    // remplacer l'ancien purge tout état interne (pas besoin de
                    // send_eof/drain — l'ancien est simplement jeté par le worker).
                    let new_video_dec = v_idx
                        .and_then(|_| ctx.build_video_decoder().ok())
                        .and_then(|d| VideoDecoder::new(d, v_tb).ok());
                    let _ = video_pkt_tx.send(VideoWorkerMsg::Reset {
                        decoder: new_video_dec, skip_until: Some(pos),
                    });
                    audio_dec = a_idx
                        .and_then(|_| ctx.build_audio_decoder().ok())
                        .and_then(|d| AudioDecoder::new(d, a_tb).ok());
                    // Décodeur local dédié à la preview post-seek-en-pause (voir plus
                    // bas) — indépendant de celui envoyé au worker.
                    preview_dec = v_idx
                        .and_then(|_| ctx.build_video_decoder().ok())
                        .and_then(|d| VideoDecoder::new(d, v_tb).ok());
                    preview_after_seek = paused;
                    v_skip_until = Some(pos);
                    a_skip_until = Some(pos);
                }
                PipelineCommand::SelectAudioTrack(track) => {
                    if let Some(&new_idx) = all_audio_idx.get(track) {
                        // Flush et reconstruit le décodeur audio pour la nouvelle piste
                        if let Some(dec) = &mut audio_dec {
                            let _ = dec.send_eof();
                            while dec.receive_frame().ok().flatten().is_some() {}
                        }
                        a_tb = ctx.format_ctx.stream(new_idx)
                            .map(|s| s.time_base().numerator() as f64 / s.time_base().denominator().max(1) as f64)
                            .unwrap_or(0.0);
                        a_idx = Some(new_idx);
                        ctx.audio_stream_idx = a_idx;
                        audio_dec = ctx.build_audio_decoder_for(new_idx)
                            .ok()
                            .and_then(|d| AudioDecoder::new(d, a_tb).ok());
                        log::info!("audio track switched → stream {new_idx}");
                    }
                }
                PipelineCommand::SelectSubtitleTrack(track_opt) => {
                    // Toutes les pistes texte sont décodées en continu — la sélection
                    // est purement un filtre côté player. Log informatif seulement.
                    log::info!("subtitle track selection: {track_opt:?}");
                }
                _ => {}
            }
        }

        if paused {
            if preview_after_seek {
                // Seek pendant la pause : décode UNE frame vidéo à la nouvelle
                // position pour que l'affichage se rafraîchisse (les paquets
                // audio/sous-titres sont ignorés).
                preview_after_seek = false;
                if let (Some(vi), Some(dec)) = (v_idx, preview_dec.as_mut()) {
                    // Un GOP long (voire un seul keyframe pour tout le fichier) peut
                    // exiger de décoder énormément de frames pour rattraper la cible
                    // depuis la keyframe précédente — pas de limite de PAQUETS totaux
                    // (audio/sous-titres dilueraient le budget), seulement un plafond
                    // temps réel généreux pour ne jamais bloquer indéfiniment le
                    // thread demuxer sur un fichier pathologique/corrompu.
                    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(800);
                    'preview: loop {
                        if std::time::Instant::now() >= deadline {
                            log::warn!("preview post-seek: délai dépassé avant d'atteindre la cible");
                            break;
                        }
                        let mut pkt = ffmpeg::Packet::empty();
                        if pkt.read(&mut ctx.format_ctx).is_err() { break; }
                        if pkt.stream() == vi {
                            let _ = dec.send_packet(&pkt);
                            let mut sent = false;
                            while let Ok(Some(frame)) = dec.receive_frame() {
                                // Même logique post-seek : atteindre le PTS cible
                                if let Some(su) = v_skip_until {
                                    if frame.pts_secs < su - 0.05 { continue; }
                                    v_skip_until = None;
                                }
                                let _ = event_tx.try_send(
                                    PipelineEvent::PositionChanged(frame.pts_secs));
                                let _ = video_tx.try_send(frame);
                                sent = true;
                            }
                            if sent { break 'preview; }
                        }
                    }
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            continue;
        }

        // Régulation du débit : queues aval pleines = on attend au lieu de lire tout
        // le fichier en avance (sinon drops massifs de frames vidéo et overflow du ring
        // audio). Pas de deadlock : pump_audio draine toujours, pump_video draine aussi
        // en Loading, et PositionChanged est émis dès qu'une frame passe.
        //
        // IMPORTANT : `video_pkt_tx` (paquets COMPRESSÉS envoyés au worker) doit être
        // inclus ici, pas seulement `video_tx` (frames DÉCODÉES en sortie du worker).
        // Sans ce check, ce thread — qui ne fait plus de décodage vidéo lui-même —
        // lit le fichier à la vitesse du disque, remplit `video_pkt_tx` (64) en
        // quelques millisecondes, puis droppe silencieusement tout paquet vidéo
        // suivant (try_send) sans jamais ralentir : le fichier entier est lu et
        // atteint EOF bien avant que la lecture réelle n'ait eu le temps de se
        // dérouler, ce qui déclenche EndOfStream quasi instantanément.
        if video_pkt_tx.is_full() || video_tx.is_full() || audio_tx.is_full() {
            std::thread::sleep(std::time::Duration::from_millis(4));
            continue;
        }

        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut ctx.format_ctx) {
            Ok(_) => {}
            Err(ffmpeg::Error::Eof) => {
                log::debug!(
                    "DBGPROBE demuxer EOF: wall={:.2}s v_pkts_forwarded={dbg_v_pkts} \
                     v_pkts_dropped(pkt_tx plein)={dbg_v_dropped} a_frames={dbg_a_frames}",
                    dbg_start.elapsed().as_secs_f64()
                );
                flush_audio_decoder(&mut audio_dec, &audio_tx);
                // Attend que le worker vidéo ait fini de vider ses frames restantes
                // avant d'annoncer EndOfStream — sinon les toutes dernières frames
                // vidéo pourraient arriver après que le player soit passé en EndOfFile
                // (pump_video n'y draine plus la queue) et ne jamais s'afficher.
                let _ = video_pkt_tx.send(VideoWorkerMsg::Eof);
                let _ = eof_ack_rx.recv_timeout(std::time::Duration::from_millis(500));
                let _ = event_tx.send(PipelineEvent::EndOfStream);
                break 'main;
            }
            Err(e) => {
                let _ = event_tx.send(PipelineEvent::Error(e.to_string()));
                break 'main;
            }
        }

        let stream_idx = packet.stream();

        if Some(stream_idx) == v_idx {
            // Décodage vidéo délégué au thread worker (jamais bloquant : sous
            // charge soutenue on droppe le paquet plutôt que d'affamer l'audio
            // lu juste en dessous, sur CE thread).
            dbg_v_pkts += 1;
            if video_pkt_tx.try_send(VideoWorkerMsg::Packet(packet)).is_err() { dbg_v_dropped += 1; }
        } else if Some(stream_idx) == a_idx {
            if let Some(dec) = &mut audio_dec {
                let _ = dec.send_packet(&packet);
                while let Ok(Some(frame)) = dec.receive_frame() {
                    // Post-seek : jeter l'audio entre la keyframe et la cible
                    if let Some(su) = a_skip_until {
                        if frame.pts_secs < su - 0.05 { continue; }
                        a_skip_until = None;
                    }
                    dbg_a_frames += 1;
                    let pos = frame.pts_secs;
                    let _ = event_tx.try_send(PipelineEvent::PositionChanged(pos));
                    // Audio : on attend si nécessaire — ne jamais dropper de frame audio
                    let _ = audio_tx.send_timeout(
                        frame,
                        std::time::Duration::from_millis(200),
                    );
                }
            }
        } else if let Some((_, ord, dec, tb)) = sub_decs.iter_mut()
            .find(|(si, _, _, _)| *si == stream_idx)
        {
            // Décode les paquets sous-titres de toutes les pistes texte
            let pts_start = packet.pts().unwrap_or(0).max(0) as f64 * *tb;
            let duration_secs = packet.duration() as f64 * *tb;
            let pts_end = pts_start + duration_secs.max(1.0);

            let mut subtitle = ffmpeg::Subtitle::new();
            if dec.decode(&packet, &mut subtitle) == Ok(true) {
                let text = collect_subtitle_text(&subtitle);
                if !text.is_empty() {
                    let _ = event_tx.try_send(
                        PipelineEvent::SubtitleLine(*ord, text, pts_start, pts_end)
                    );
                }
            }
        }
    }

    Ok(())
}

/// Extrait le texte brut d'un paquet subtitle ffmpeg (SRT/ASS/WebVTT/HDMV-PGS partiellement).
fn collect_subtitle_text(subtitle: &ffmpeg::Subtitle) -> String {
    use ffmpeg::subtitle::Rect;
    let mut parts: Vec<String> = Vec::new();
    for rect in subtitle.rects() {
        match rect {
            Rect::Text(t) => {
                let raw = t.get();
                // Supprime les balises HTML basiques (<i>, <b>, etc.) souvent présentes en SRT
                let clean = strip_basic_tags(raw);
                if !clean.trim().is_empty() {
                    parts.push(clean.trim().to_string());
                }
            }
            Rect::Ass(a) => {
                // Le decoder subrip/ass de FFmpeg produit soit une ligne complète
                // "Dialogue: Layer,Start,End,Style,Name,..,Text", soit directement les
                // champs après le timing selon le codec. On extrait le texte après le
                // 8e champ « Effect » quand le préfixe Dialogue est présent, sinon on
                // prend tout. Robuste aux deux formes.
                let line = a.get();
                let body = line.strip_prefix("Dialogue:").unwrap_or(line);
                // Une ligne Dialogue a 9 champs avant le texte ; les événements bruts
                // du décodeur subrip n'ont que Layer + texte. On compte les virgules.
                let comma_count = body.matches(',').count();
                let text = if comma_count >= 9 {
                    body.splitn(10, ',').nth(9).unwrap_or("")
                } else {
                    // Format "ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text"
                    // (subrip → ass : 8 champs avant le texte)
                    body.splitn(9, ',').last().unwrap_or(body)
                };
                let clean = strip_ass_overrides(text.trim());
                if !clean.trim().is_empty() {
                    parts.push(clean.trim().to_string());
                }
            }
            _ => {} // Rect::Bitmap (PGS/VOBSUB) — pas de texte extractible
        }
    }
    parts.join("\n")
}

fn strip_basic_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut inside = false;
    for c in s.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

fn strip_ass_overrides(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    // Remplace les sauts de ligne ASS \N et \n
    out.replace("\\N", "\n").replace("\\n", "\n")
}

fn flush_audio_decoder(
    audio_dec: &mut Option<AudioDecoder>,
    audio_tx:  &Sender<DecodedAudioFrame>,
) {
    if let Some(dec) = audio_dec {
        let _ = dec.send_eof();
        while let Ok(Some(f)) = dec.receive_frame() {
            let _ = audio_tx.send_timeout(f, std::time::Duration::from_millis(200));
        }
    }
}
