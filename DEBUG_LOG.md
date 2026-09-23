# ForgePlayer — Journal de débogage

Mis à jour en continu pendant la campagne de test sur PC réel (audio matériel).
Format par entrée : `[STATUT] Zone — description`. STATUT ∈ {FIXED, OPEN, TESTED-OK, TODO}.

---

## Bugs corrigés cette session (v1.4.7 → v1.5.0) — 4K/HDR audio cuts + stutter, renommage ForgePlayer

- [FIXED][CRITIQUE] **"HW accel" placebo depuis toujours** — `hw_accel::apply_to_codec()` ne faisait qu'activer le threading logiciel FFmpeg (`ctx.set_threading`), jamais de vrai `hw_device_ctx`/`get_format`. Tout flux, y compris 4K HEVC/AV1 10-bit HDR, décodait 100% en logiciel quel que soit le réglage d3d11va/dxva2 affiché dans Paramètres. Fix : vrai décodage D3D11VA/DXVA2 via `av_hwdevice_ctx_create` + callback `get_format` (motif officiel `hw_decode.c`), repli logiciel silencieux si négociation/init échoue. `decoder/video.rs` rapatrie les frames GPU en mémoire système (`av_hwframe_transfer_data`) avant le pipeline SwsContext/extract_planes existant, inchangé.
- [FIXED][CRITIQUE] **Démux + décodage vidéo + décodage audio sur UN SEUL thread partagé** — un décodage vidéo 4K/HDR lourd (forcément logiciel vu le bug ci-dessus) bloquait le décodage audio suivant sur le même thread → coupures audio. Fix : décodage vidéo déplacé sur un thread dédié (`video_worker.rs`, nouveau), paquets vidéo transmis via canal borné non-bloquant (`try_send`, drop plutôt que d'affamer l'audio).
- [FIXED][HAUTE] **Régression découverte pendant le test du fix ci-dessus** : sans le décodage vidéo pour ralentir ce thread, la lecture des paquets fonçait à la vitesse du disque et atteignait l'EOF réel du fichier en une fraction de la durée réelle. Fix : la porte de régulation (`if ... is_full() { sleep }`) surveille aussi la nouvelle queue de paquets vidéo vers le worker.
- [FIXED][HAUTE] **Deuxième régression, même campagne** : même une fois la lecture correctement cadencée, `PipelineEvent::EndOfStream` basculait direct en `PlayerState::EndOfFile`, qui arrête `pump_video`/déclenche la boucle suivante — le décodage matériel finit de lire+décoder tout le fichier bien avant que la lecture temps réel des frames déjà en file n'ait rattrapé la fin. `Player::maybe_finish_playback()` attend maintenant que `position` rattrape `duration` (ou un délai de grâce de 3s) avant de vraiment terminer.
- [FIXED] Réglage "HW accel" des Paramètres jamais lu nulle part (mort depuis toujours) — câblé `AppConfig.hw_accel` → `Player` → `MediaPipeline::launch` → `run_demuxer`, "none" est maintenant une vraie échappatoire si un pilote GPU pose problème.
- [FIXED] Échec d'ouverture du décodeur AUDIO tuait TOUTE la lecture (vidéo comprise) via `?` — pouvait ressembler à "codec manquant" sur un fichier par ailleurs lisible. Dégrade maintenant en vidéo muette + avis OSD non-bloquant (`PipelineEvent::Warning`, nouveau).
- [FIXED] `RUST_LOG` totalement ignoré — `main.rs` appelait `filter_level(Info)` sans jamais lire l'env var. `Env::default_filter_or("info")` à la place.
- [FIXED][MINEUR] Qualité de conversion chroma 4K : `Flags::BILINEAR` → `BICUBIC` pour la conversion de format (4:2:2/4:4:4→4:2:0, coût quasi nul à cette taille).
- [FIXED] `patches/ffmpeg-next` était un gitlink de sous-module orphelin (aucun `.gitmodules`, aucun `.git` imbriqué) — jamais réellement suivi par git, confirmé par l'absence documentée dans les clones frais. Reconverti en fichiers normaux.
- **Vérifié réel (RTX 3070)** : clip 3840×2160 HEVC Main10 HDR10 + EAC3 5.1 généré ex-professo (`ffmpeg testsrc2` + tags HDR10 réels, pas juste des métadonnées bidon). `HW accel initialisé: D3D11Va` confirmé aux logs, cadence temps réel confirmée par sonde wall-clock (DBGPROBE), et un clip 4K SDR équivalent s'affiche correctement image par image (couleurs, mouvement) via capture d'écran réelle. Le clip HDR s'affichait quasi-noir — testsrc synthétique tagué PQ mais pas réellement gradé HDR, limitation déjà connue/documentée (v1.4.5), pas une régression (confirmé en comparant au clip SDR qui traverse le même nouveau code et s'affiche bien).
- **Renommage complet OmniPlayer → ForgePlayer** : titre fenêtre, chaînes About/logs, package Rust (`crates/omni-player` → `crates/forgeplayer`, binaire `forgeplayer.exe`), module Go, dossier config (`%APPDATA%/ForgePlayer`), installeur (même AppId → mise à jour in-place des installs existants), scripts build/launch, docs. Noms des crates bibliothèques internes (`omni-core`, `omni-renderer`, `omni-audio`) volontairement inchangés — détail d'implémentation non visible utilisateur.

## Bugs corrigés cette session (v1.4.0 → v1.4.5)

- [FIXED] **HDR décodé mais toujours tronqué en 8-bit avant rendu** (limitation notée en v1.4.4, corrigée ici). `video.rs` forçait `target_fmt = YUV420P` pour TOUTE source, HDR 10-bit compris. Fix complet bout-en-bout :
  - `PixelFormat::Yuv420p10le` ajouté ; `desired_target()` préserve `YUV420P10LE` pour toute source 10-bit (`YUV420P10LE/BE`, `422P10`, `444P10`, `P010`), YUV420P sinon (comportement SDR inchangé).
  - `extract_planes` décale chaque échantillon 16-bit de 6 bits (convention FFmpeg 10LE bits bas → convention P010 bits hauts) pour que la normalisation GPU standard retombe sur le bon ratio.
  - Textures GPU en `R16Unorm` pour le HDR (au lieu de `R8Unorm`) — nécessite la feature wgpu `TEXTURE_FORMAT_16BIT_NORM`, demandée explicitement à la création du device (`main.rs`, `device.features() & optional` — ne casse rien si l'adaptateur ne l'a pas) ; repli automatique en 8-bit (downsample par troncature du byte haut) si la feature n'est pas dispo, jamais de crash.
  - Deuxième passe de rendu : `VideoRenderer::render_to_offscreen` (YUV→RGB PQ vers texture `Rgba16Float` intermédiaire) puis `HdrTonemapper` (déjà existant mais jamais branché — PQ inverse EOTF + Reinhard/ACES/Hable) vers le swapchain SDR. Sélection du chemin (direct SDR vs deux passes HDR) via `OmniApp.video_is_hdr` (AtomicBool, mis à jour à chaque frame affichée, réinitialisé dans `open_file` pour éviter tout état qui fuite d'un fichier à l'autre).
  - **Testé avec un vrai fichier HDR10** généré via `ffmpeg -x265-params colorprim=bt2020:transfer=smpte2084:...:hdr10=1` (HEVC, yuv420p10le, BT.2020, PQ réel — pas juste des métadonnées bidon). Ouverture, lecture, badge HDR, changement de mode tonemap (Reinhard/ACES/Hable) via Paramètres : tout fonctionne sans crash. Bascule HDR→erreur→SDR dans la même session testée explicitement (risque de fuite d'état GPU) : transition propre, badge HDR disparaît, rendu SDR normal juste après (vraie vidéo 1080p60 testée).
  - Limitation résiduelle assumée : le tonemapping opère sur un contenu synthétique PQ-tagué mais pas réellement gradé HDR (pas de fichier HDR gradé disponible pour comparaison visuelle fine des courbes Reinhard/ACES/Hable) — le pipeline est mécaniquement correct (vérifié par la chaîne de conversions), mais la qualité perceptuelle sur du vrai contenu HDR gradé n'est pas validée visuellement.

- [OPEN][MINEUR] **`file://` non supporté dans le dialogue "Ouvrir une URL"** — schéma listé comme valide (`VALID_SCHEMES` dans `url_dialog.rs`) mais FFmpeg/libavformat sur Windows n'accepte pas `file:///C:/...` (erreur "No such file or directory"). Trouvé incidemment en testant la bascule HDR→SDR. Pas corrigé (hors sujet de cette session, contournable via Ctrl+O). Si un jour prioritaire : soit strip `file://` et repasse en chemin natif avant d'appeler `player.open()`, soit retire `file://` de `VALID_SCHEMES` si jamais utile en pratique (Ctrl+O couvre déjà ce cas).

## Bugs corrigés session précédente (v1.4.0 → v1.4.4)

- [FIXED][CRITIQUE] **La barre de progression (seek bar) ne faisait RIEN** — cliquer/glisser dessus redessinait juste le curseur pour la frame courante puis revenait à l'ancienne position la frame suivante ; aucun vrai seek n'était jamais déclenché. Cause : `seek_bar()` (`controls.rs`) construit sa zone interactive via `ui.allocate_exact_size(..., Sense::click_and_drag())` — ce `Response` "brut" ne marque JAMAIS `changed=true` tout seul (contrairement à un `Slider`/`Checkbox` standard qui appelle `mark_changed()` en interne). Le code appelant testait `if seek_bar(...).changed() { *seek_out = Some(pos); }`, qui restait donc perpétuellement faux. **RETRACTATION** : j'avais précédemment marqué "seek bar clic souris" comme TESTED-OK sur la foi d'un test bâclé (vidéo déjà en lecture → l'horodatage progressait naturellement, confondu avec un vrai saut). Signalé par Momo en test réel. Fix : `resp.mark_changed()` ajouté explicitement après mutation de `pos`.
- [FIXED][HAUTE] **Frame figée après un seek pendant la pause, sur fichier à GOP long/keyframe unique.** Le mécanisme de preview post-seek-en-pause (`demuxer.rs`) décodait jusqu'à trouver la frame cible, mais plafonnait à 400 paquets TOTAUX (vidéo+audio+sous-titres confondus). Sur un fichier avec une seule keyframe pour tout le fichier (repéré sur `test_mkv_long.mkv`, généré par une session précédente), atteindre une cible à 6s depuis l'unique keyframe à t=0 exige de décoder ~180 frames vidéo, largement au-delà du budget dilué par les paquets audio/sous-titres interleaved → recherche abandonnée silencieusement, ancienne image restée affichée indéfiniment (position/barre correctes, mais IMAGE figée). Fix : budget temps réel (800ms) au lieu d'un compte de paquets, insensible au ratio audio/vidéo/sous-titres.
- [FIXED] **File browser interne rejetait toute extension hors d'une liste blanche codée en dur** — contredit "compatible avec n'importe quel format" : FFmpeg sniffe le contenu réel et se moque de l'extension, mais le bouton "Ouvrir"/double-clic restaient désactivés pour tout fichier dont l'extension n'était pas dans `SUPPORTED_EXTENSIONS` (même très longue, jamais exhaustive : fichier renommé, extension régionale/obscure, absente). Fix : `is_media_file` autorise tout sauf une petite liste noire de types manifestement non-média (exe/dll/txt/zip/pdf/…) ; testé avec un .mp4 renommé en `.xyz123`, ouverture et lecture normales.

- [FIXED] Ctrl+L (ouvrir URL) déclenchait AUSSI le raccourci brut `L` (cycle mode répétition) — `k_l` ne vérifiait pas `!ctrl`. Même défaut sur Ctrl+P (ouvrait le panneau playlist ET appelait `playlist_prev()`). Trouvé par capture d'écran : l'OSD "Répétition : ×1" apparaissait derrière le dialogue URL fraîchement ouvert.
- [FIXED] Échap ne fermait aucun dialogue modal (URL, file browser, paramètres) — `k_esc` ne gérait que la sortie plein écran. Pire : le dialogue URL force le focus clavier du champ texte à CHAQUE frame (`resp.request_focus()` sans condition), donc `wants_keyboard_input()` reste vrai en continu → même en ajoutant la gestion d'Échap dans `handle_keyboard`, elle n'aurait jamais été atteinte pour ce dialogue précis. Fix : Échap lu directement dans `url_dialog.rs` (en amont du filtre global) ; ajout du cas fermeture file browser/paramètres dans `handle_keyboard`.

- [FIXED] Horloge pilotée par le curseur de décodage (~14s d'avance) au lieu de l'audio réellement joué → vidéo en avance sur l'audio.
- [FIXED] Ring audio (8s) pas purgé au seek/changement de fichier → désync multi-secondes après action utilisateur.
- [FIXED] `av_seek_frame` : ancien seek `format_ctx.seek(ts, ts..)` renvoyait EPERM sur MP4 → seek cassé.
- [FIXED] Sous-titres intégrés jamais affichés (`update_subtitle` écrasait `current_subtitle=None` en boucle).
- [FIXED] Parsing ASS/subrip : mauvais nombre de champs avant le texte (8 vs 9) → texte vide.
- [FIXED] Fin de fichier / répétition ×1 : pipeline mort après EOF, seek(0) ne faisait rien → lecteur figé.
- [FIXED] Erreurs demuxer jamais remontées à l'UI (log seul).
- [FIXED] Volume/vitesse sauvegardés mais jamais relus au démarrage.
- [FIXED] CRT dynamique (vcruntime140 manquant) → crash au lancement sur PC sans VC++ Redist. Passé en `+crt-static`.
- [FIXED] Son accéléré permanent : flux CPAL ouvert avec canaux natifs du device (ex. 6 en 5.1/7.1) alors que le pipeline downmixe toujours en stéréo → ring vidé N/2× trop vite. Flux forcé en stéréo.
- [FIXED] Fallback silencieux vers échantillons bruts si le resampler échoue à se créer → lecture à mauvaise fréquence indéfiniment.
- [FIXED] Deadlock démultiplexeur si pas de périphérique audio (`pump_audio` ne vidait pas la file du pipeline) → vidéo figée + seek mort après ~30s.
- [FIXED] Dérive horloge sans audio : `PositionChanged` recalait l'horloge sur chaque frame décodée au lieu de la laisser tourner en roue libre → retard croissant.

## Bugs ouverts / suspects (à vérifier sur ce PC)

- [OPEN] Aucun test réel de la sortie AUDIO multi-canaux (5.1/7.1) — fix v1.4.1 pas vérifié sur un vrai device surround (aucun disponible ici). Vérifier au moins que le stream s'ouvre correctement en stéréo forcé sans erreur sur ce PC (device standard).
- [OPEN] Piste audio multiple (`next_audio_track`) jamais testée avec un vrai fichier multi-pistes.
- [OPEN] Chapitres (navigation, marqueurs seekbar) jamais testés avec un vrai fichier ayant des chapitres.
- [OPEN] HDR (badge, tonemapping) jamais testé avec un vrai fichier HDR.
- [OPEN] Lecture réseau (HTTP/RTMP/HLS) mentionnée au README mais jamais testée.
- [OPEN] Recherche de sous-titres OpenSubtitles / métadonnées TMDB — nécessite clés API, jamais testé.
- [OPEN] Bibliothèque média (indexeur Go, port 18081) — jamais testée.
- [OPEN] Sous-titres bitmap (PGS/VOBSUB) — non supportés (connu, pas un bug).
- [OPEN] D3D11VA zero-copy GPU pipeline — non implémenté (connu, perf uniquement).

## Fonctionnalités testées sur PC hôte (audio réel) — 2026-07-19

- [TESTED-OK] Piste audio multi-track (touche A) — `test_multiaudio.mp4` (fr/eng), badge "♪ eng" correct après switch, process stable.
- [TESTED-OK] Chapitres — titre "Milieu" affiché, marqueurs jaunes sur seekbar aux bons offsets, boutons ⏮/⏭ apparaissent.
- [TESTED-OK] Sous-titre externe adjacent auto (.srt à côté du .mp4) — "Sous-titre externe DEUX" affiché au bon timing.
- [TESTED-OK] Vitesse [ / ] — 1×→1.25×→1.5×→1.25×→1×, badge vitesse correct à chaque étape, aucune anomalie après retour à 1×.
- [TESTED-OK] Volume ↓↓ + Mute (M) — OSD "Muet" + icône haut-parleur barrée corrects.
- [TESTED-OK] Format image (W) — Fit→Fill→Stretch→Fit, rendu visuellement correct (vidéo réellement étirée en mode Stretch, sans letterbox).
- [TESTED-OK] Plein écran (F) puis Échap — bascule réelle (barre de titre disparaît, résolution pleine 1920×1080, reconnu par l'overlay NVIDIA), retour fenêtré OK.
- [TESTED-OK] Overlay infos (I) — panneau complet et exact (conteneur, codec, résolution, débit, piste audio, espace couleur, vitesse, format, buffer, position clock).
- [TESTED-OK] Mode boucle (L) — Off→×1→All, icône + OSD corrects.
- [TESTED-OK] Visionneuse image PNG — centrée, badge résolution/qualité correct, échelle 100%.
- [TESTED-OK] EOF + replay (Espace) — confirmé dans campagne VM précédente, revalidé implicitement ici (process stable après tous les changements d'état).

## Testé — round 2 (dialogues, playlist, seekbar souris) — 2026-07-19

- [TESTED-OK] Ouverture URL (Ctrl+L) — dialogue s'ouvre, plus d'effet de bord loop, Échap ferme proprement.
- [TESTED-OK] File browser (Ctrl+O) — dialogue s'ouvre, Échap ferme proprement.
- [TESTED-OK] Paramètres (menu Outils > Paramètres, clic souris direct) — formulaire complet et cohérent (accel matérielle, tone mapping HDR, volume défaut, langue sous-titres, ports services Go, bibliothèque médias), Échap ferme proprement.
- [TESTED-OK] Playlist (Ctrl+P) — panneau s'ouvre avec l'entrée courante listée, plus d'effet de bord playlist_prev.
- [TESTED-OK] Seek bar clic souris (pas seulement clavier) — clic à ~60% de la barre saute correctement à la position correspondante.

## Nouveautés v1.4.4

- **Playlist — enregistrer/charger (M3U8/M3U)** : nouveau module `playlist_io.rs` (`save_m3u`/`load_m3u`), boutons 💾/📂 dans le panneau playlist + menu Fichier. Résout chemins relatifs par rapport au dossier du .m3u, ignore silencieusement les entrées introuvables (playlist déplacée d'une machine à l'autre), supporte les URL réseau telles quelles. 3 tests unitaires (round-trip, entrées manquantes, chemins relatifs) — tous verts.
- **Bouton "+ Ajouter" du panneau playlist** — ne faisait RIEN (juste un commentaire "s'ouvre via l'app", jamais implémenté). Fix : ouvre maintenant le file browser normalement.
- **Compatibilité codecs/formats** : validé avec le build FFmpeg complet actuel (527 décodeurs, BtbN full/GPL) au-delà de H.264/AAC déjà testés — **VP9+Opus (WebM)**, **AV1+Vorbis (MKV)**, **FLAC** tous lus sans erreur, badge codec correct affiché. Le décodage ne dépend jamais de l'extension (FFmpeg sniffe le contenu), donc la couverture de codecs est déjà large par construction ; le vrai verrou était le filtre extension du file browser (corrigé ci-dessus).

## Reste à tester (pas encore couvert)

- [ ] Playlist : ajout multiple (+Ajouter), navigation N/P avec plusieurs éléments, Vider — bouton +Ajouter maintenant fonctionnel mais pas retesté avec plusieurs fichiers
- [ ] Drag & drop réel (fichier média + sous-titre) — mécanisme OS, difficile à automatiser ; code lu et cohérent (`handle_drop` dans app.rs), risque faible
- [ ] Redimensionnement fenêtre / changement DPI — TESTÉ OK (petite 500×350 et grande 1400×900, rendu correct)
- [ ] Fermeture propre + relecture config — TESTÉ OK (WM_CLOSE sauvegarde recent_files correctement, revérifié via config.json)
- [ ] Fichiers récents — TESTÉ OK (menu peuplé, sous-menu affiche les 5 dernières entrées)
- [ ] Chargement sous-titre manuel (Fichier > Charger sous-titre…)
- [ ] Effacer sous-titre (Fichier > Effacer sous-titre)
- [x] ~~HDR — pipeline 8-bit uniquement~~ **CORRIGÉ v1.4.5** — voir section bugs corrigés ci-dessus. Vrai pipeline 10-bit bout-en-bout (décodage → textures R16Unorm → tonemap PQ) implémenté et testé avec fichier HDR10 réel (HEVC BT.2020 PQ).
- [ ] Lecture réseau (HTTP/HLS) — non testé
- [ ] Recherche sous-titres OpenSubtitles / TMDB — nécessite clés API
- [ ] Sortie audio 5.1/7.1 réelle — aucun device surround disponible ici pour vérifier le fix v1.4.1 sur vrai matériel

---

## v1.6.0 (2026-09-23) — 4K/2K saccadé + HDR « hyper éblouissant »

### Corrigé

- [FIXED] **HDR brûlé (cause principale)** — `assets/shaders/hdr_tonemap.wgsl` définissait `pq_to_linear()` mais ne l'appelait JAMAIS. Le signal PQ (0..1, non linéaire) était directement multiplié par `exposure / max_luminance * 10000` (soit ×10 avec les valeurs par défaut) puis tone-mappé : tout ce qui dépassait 10 % de code PQ saturait à blanc. Shader réécrit sur la chaîne standard (celle de VLC/libplacebo) : EOTF PQ ou HLG → luminance absolue en nits → normalisation sur le blanc diffus 203 nits (ITU-R BT.2408) → tone mapping (Reinhard étendu / ACES / Hable) → gamut BT.2020 → BT.709 → encodage sRGB exact.
- [FIXED] **Tout flux 10-bit était traité comme du HDR** — `video_is_hdr` venait de `PixelFormat::is_hdr10bit()` (profondeur de bits). Un fichier 10-bit BT.709 (HEVC Main10 SDR, très courant) partait donc dans le tone mapping PQ et ressortait brûlé. Le chemin HDR suit maintenant la fonction de transfert réelle du flux (`VideoStreamInfo::transfer` : 0 SDR, 1 PQ, 2 HLG), lue dans les métadonnées.
- [FIXED] **HLG traité comme du PQ** — la courbe HLG est maintenant appliquée séparément (OETF inverse + OOTF système gamma 1.2).
- [FIXED] **Conversion CPU par frame sur tout le décodage matériel (cause des saccades 4K/2K)** — les frames rapatriées du GPU (NV12 en 8-bit, P010LE en 10-bit) passaient systématiquement par `SwsContext` (NV12→YUV420P, P010→YUV420P10LE) puis, en 10-bit, par une passe supplémentaire `shift_10_to_16` avec allocation d'un tampon de 24 Mo — plusieurs millisecondes de CPU mono-thread par image en 4K. NV12 et P010 partent maintenant tels quels au GPU (textures RG semi-planaires, `TexLayout`), comme le fait VLC : zéro conversion, zéro allocation supplémentaire.
- [FIXED] **Frames futures jetées au lieu d'attendre** — `video_worker` faisait `try_send` sur la file de frames décodées : quand le décodage prenait de l'avance, les images étaient perdues (102 sur 480 mesurées sur un 4K HDR 24p) alors qu'il suffisait d'attendre que l'UI en consomme une. Remplacé par `send_timeout(100 ms)` (contre-pression, le délai de garde évite tout blocage en pause/fermeture) : **0 frame perdue sur 480**.

### Vérifié (PC dev, RTX 3070, D3D11VA actif)

- `real_hdr10_4k_24p_av.mp4` — vrai contenu gradé PQ (Netflix *Chimera*, CC-BY, ré-encodé HEVC Main10 BT.2020/PQ 45 Mbps) + piste audio : 40 s de lecture, `pos` suit `wall` à ±10 ms, **1 frame perdue sur 960**, audio maître, tampon audio 3,5 s stable. Capture d'écran : image correctement exposée, plus de brûlure.
- `hdr10_4k_motion.mp4` (4K HDR10 24p, 40 Mbps) : **0 frame perdue sur 480**, `pos == wall` tout du long.
- `hdr10_4k.mp4` (rampe PQ noir→blanc 4K) : dégradé complet visible de bout en bout (avant : quasi tout blanc).
- `sdr10_4k.mp4` (4K **10-bit BT.709 SDR**) : aucun badge HDR, aucun tone mapping, couleurs normales — la régression « 10-bit = HDR » ne se produit plus.

### Calage sur VLC (2026-09-23, itération 2 de la boucle)

- [FIXED] **Tone mapping appliqué canal par canal** — chaque composante était compressée séparément, donc le canal dominant saturait avant les autres et les aplats colorés se délavaient (aplat bleu mesuré à +0,17 par rapport à VLC). Le tone mapping porte maintenant sur la LUMINANCE seule, la chrominance suit le même rapport (méthode libplacebo/VLC).
- [FIXED] **Courbe par défaut trop claire** — ACES et Hable sont des courbes *scene-referred* : elles remontent aussi les tons moyens. Courbe de VLC relevée expérimentalement sur le même fichier (rapport luminance source normalisée 203 nits → luminance affichée) : 0,037→0,034 · 0,071→0,055 · 0,343→0,257 · 1,95→0,725 · 5,58→1,0. C'est exactement du **Reinhard étendu** avec le pic à `max_luminance/203`. Devenu le mode par défaut (`tonemap_mode = 0`).
- **Mesure** (mêmes plan fixe HDR10, fenêtres capturées, luminance linéarisée, écart moyen des quantiles p5→p90 par rapport à VLC) : ACES par canal **0,0505** · knee neutre **0,0576** · **Reinhard étendu 0,0162**. Quantiles : VLC p50 0,257 / p75 0,725 — ForgePlayer p50 0,254 / p75 0,703.
- Modes ACES / Hable / Neutre restent disponibles dans Paramètres (désormais eux aussi luminance-only).
- Méthode réutilisable : plan fixe de 5 s (`still_hdr_5s.mp4`), capture des deux fenêtres par `pilote.py`, dé-gamma sRGB puis comparaison des quantiles — insensible au décalage de cadrage entre les deux lecteurs.

### Pic de tone mapping lu dans le fichier (2026-09-23, itération 3 de la boucle)

- [FIXED] **Le pic de tone mapping était une constante de configuration** (1000 nits) quelle que soit la source. Un master 4000 nits se faisait écraser les hautes lumières, un master 600 nits en gardait trop. Le pic vient maintenant du fichier : **MaxCLL** s'il est présent (pic réel mesuré à l'encodage), sinon la **luminance max de l'écran de mastering**, sinon repli sur le réglage.
- **Piège** : les métadonnées HDR10 d'un encodage x265 `hdr10=1` ne sont PAS dans `codecpar` (aucune boîte `mdcv`/`clli` au niveau du conteneur), seulement dans les SEI du flux. Lire `AVCodecParameters::coded_side_data` ne suffit donc pas — la sonde décode aussi la première image (budget borné à 60 paquets) et lit `av_frame_get_side_data`. Les deux chemins sont couverts.
- **Piège** : `ffmpeg-sys-next` ne binde pas `libavutil/mastering_display_metadata.h` — `AVMasteringDisplayMetadata` et `AVContentLightMetadata` sont redéclarées en `#[repr(C)]` dans `probe.rs`. Valeurs hors de 100..10000 nits ignorées (métadonnée cassée).
- **Mesure** : deux rampes PQ identiques encodées avec `max-cll=1000` et `max-cll=4000`. Journal : « pic annoncé par le fichier = 1000 nits » puis « = 4000 nits ». Rendu : hautes lumières tenues 0,038 plus bas en 4000 nits (x=1,0 : 0,888 → 0,850 ; x=0,8 : 0,762 → 0,734), tons moyens quasi inchangés (−0,006 à mi-course) — exactement le comportement attendu d'un pic plus élevé.
- Un fichier SDR n'émet aucune ligne HDR (vérifié sur `sdr10_4k.mp4`).

### HLG validé sur un vrai fichier HLG (2026-09-23, itération 4 de la boucle)

- Fichier de test produit par conversion réelle (pas un simple re-tag) : `zscale=t=linear:npl=1000,zscale=t=arib-std-b67:p=bt2020:m=bt2020nc` puis x265 `transfer=arib-std-b67` → `still_hlg_5s.mp4` et `ramp_hlg.mp4`.
- [TESTED-OK] Détection : journal `HDR: transfert=2 (1=PQ, 2=HLG)` — la courbe HLG est bien sélectionnée, pas la PQ.
- [TESTED-OK] Rendu comparé à VLC sur le même plan fixe : écart moyen des quantiles p5→p90 = **0,0120** (PQ : 0,0162). Hautes lumières légèrement plus claires que VLC (+0,041 à p90), tons moyens identiques (−0,001 à p50).
- Les liserés verts sur les fils et la bande pâle à droite sont **dans la source** (artefacts de la conversion zscale) : VLC affiche exactement les mêmes. Pas un défaut du lecteur.
- Aucun changement de code nécessaire : la voie HLG écrite à l'itération 1 est correcte.

### Plage de couleur complète (JPEG/PC) (2026-09-23, itération 5 de la boucle)

- [FIXED] **Le shader supposait toujours la plage limitée** : offset de luma 16/255 et facteurs 255/219 (luma) / 255/224 (chroma) codés en dur dans les trois matrices. Un fichier full range (`color_range=pc`, courant en capture d'écran, webcam, GIF/MJPEG ré-encodé) voyait donc ses noirs écrasés et ses blancs écrêtés.
- Les matrices ne sont plus écrites à la main : `ColorUniforms::from_coeffs(kr, kb, full_range)` les dérive des coefficients de luminance (BT.601 0,299/0,114 · BT.709 0,2126/0,0722 · BT.2020 0,2627/0,0593) et de la plage. L'offset de luma passe par `color.offset.y` (0 en full range, 16/255 en limited).
- `color_range` est lu dans la sonde (`ffmpeg::color::Range::JPEG`) et suit jusqu'au shader.
- **Mesure** : même image encodée deux fois, une en `pc` et une en `tv`. Quantiles de luminance (p5 / p50 / p75 / p95) :
  - image source PNG (vérité terrain) : 0,153 / 0,338 / 0,480 / 0,798
  - ForgePlayer sur le fichier **full range** : 0,139 / 0,322 / 0,463 / 0,738
  - ForgePlayer sur le fichier **limited** : 0,139 / 0,322 / 0,463 / 0,738 (écart moyen entre les deux rendus : 0,0045)
  - VLC sur le fichier **full range** : 0,085 / 0,334 / 0,538 / **1,000**
- Les deux rendus de ForgePlayer sont identiques et collent à la source ; **VLC, lui, ignore le drapeau `pc`** sur ce fichier (noirs écrasés, hautes lumières écrêtées). Sur ce point précis le lecteur est plus juste que VLC.

### Passe CPU supprimée sur le 10-bit logiciel (2026-09-23, itération 6 de la boucle)

- [FIXED] Le 10-bit décodé en logiciel subissait `shift_10_to_16` : relecture de tout le plan, décalage de 6 bits échantillon par échantillon et **réallocation de 24 Mo par image en 4K**, uniquement pour que le shader retombe sur la bonne échelle. Les plans partent maintenant tels que FFmpeg les produit (valeur 10 bits dans les bits bas) et le shader applique le facteur `65535/1023` (`color.offset.z`) — une multiplication par texel, gratuite sur GPU.
- Bonus de justesse : l'ancien décalage donnait `valeur × 64 / 65535` = division par 1023,98 au lieu de 1023 (−0,06 %). Le facteur est maintenant exact.
- Repli 8 bits (GPU sans `TEXTURE_FORMAT_16BIT_NORM`) adapté : `narrow_16_to_8` prend le décalage en paramètre (2 pour le 10-bit aligné bas de FFmpeg, 8 pour le P010 aligné haut du décodage matériel).
- **Mesure de justesse** : même image fixe encodée en 8 bits et en 10 bits, quantiles p5→p95 — rendu 10 bits **0,002** d'écart avec le rendu 8 bits, et 0,026 avec le PNG source (même écart systématique que le 8 bits, dû à la capture et au ré-encodage). Aucune régression visuelle.
- **Mesure de coût CPU : non concluante** sur cette machine. Secondes CPU pour 20 s de lecture 4K 10-bit en décodage 100 % logiciel : 21,67 / 22,00 avant, 23,16 / 16,66 après — le décodage HEVC logiciel domine et le bruit dépasse le gain. Le gain reste structurel (une passe plein cadre et une allocation de 24 Mo par image en moins) ; il ne concerne que le chemin logiciel, le décodage matériel n'y passait déjà plus.

### Lecture 4K HDR continue de 5 minutes (2026-09-23, itération 7 de la boucle)

- [TESTED-OK] Fichier de 330 s en 4K HDR10 PQ 10-bit avec piste audio (`long_hdr_4k.mp4`, 1,6 Go), décodage matériel D3D11VA, lecture d'une traite.
- **Aucune frame perdue** : 7 911 paquets vidéo transmis, 7 911 décodés, **0 droppée** côté worker et 0 côté démultiplexeur. 15 462 frames audio.
- **Aucune coupure audio** : tampon audio jamais sous **2,32 s** (moyenne 3,44 s, maximum 3,65 s) sur 102 relevés ; `audio_master=true` sur la totalité du test, aucun relevé sous 0,5 s.
- **Dérive audio/vidéo** : `pos - wall` passe de −0,010 s à +0,130 s en 317 s, soit **+0,041 %** — c'est l'écart de cadence entre l'horloge du périphérique audio (qui fait référence) et l'horloge système, pas une dérive de synchronisation : la vidéo suit l'horloge audio, donc l'image reste calée sur le son. Inaudible et invisible.
- Aucun avertissement ni erreur dans le journal hormis l'absence des services Go optionnels (ports 18080/18081).

### Seek sur 4K HDR : image figée en pause, gel de plusieurs secondes en lecture (2026-09-23, itération 8 de la boucle)

- [FIXED] **Un seek en pause ne rafraîchissait pas l'image** sur un fichier 4K : les deux captures avant/après la touche → étaient rigoureusement identiques (écart 0,0000) et le journal affichait `preview post-seek: délai dépassé avant d'atteindre la cible`. Cause : après `av_seek_frame`, le décodage repart de l'image clé qui précède la cible et l'ancien code décodait TOUTES les images intermédiaires sans rien afficher. Sur ce fichier les images clés sont espacées de 10,4 s (encodage NVENC) : rattraper 1 à 4 s de 4K HEVC 10-bit dépasse largement le budget de 800 ms de la preview.
- [FIXED] **Même cause en lecture** : après quelques sauts, la position restait figée (26,91 s sur deux relevés consécutifs) avec `buffered=0.00` et `raw_audio_pos=None` — plusieurs secondes d'image gelée et de son coupé.
- Correctif : la première image décodée après un seek décide. Si l'image clé est à plus de **1 s** (`SEEK_CATCHUP_MAX_SECS`) de la cible, elle s'affiche immédiatement au lieu d'être rattrapée — c'est ce que font VLC et mpv sur les sauts au clavier. En dessous du seuil, le rattrapage exact est conservé. L'audio et la preview appliquent le même seuil, sinon le son partirait en avance de tout le GOP.
- **Mesure après correctif** (mêmes manipulations, mêmes fichiers) :
  - seek en pause : écart entre la capture avant et après = **0,0756** (était 0,0000), **0 dépassement de délai** (était 1 par seek).
  - 5 sauts en lecture : **aucun relevé avec tampon audio vide**, position jamais figée (elle avance à chaque relevé : 21,07 → 31,41 → 40,10 → 31,17 → 34,17 → 42,84…), tampon redescendu à 0,04 s puis remonté à 3,5 s.
  - Journal : `seek: image clé à 40,06 s pour une cible à 44,24 s — affichage immédiat plutôt qu'un rattrapage de 4,18 s`.
- **Compromis assumé** : sur un fichier à GOP très long (10,4 s ici), un saut peut se caler jusqu'à ~10 s avant la position demandée. C'est le comportement des autres lecteurs ; l'alternative est un gel de plusieurs secondes.

### Version 1.6.0 alignée partout (2026-09-23, itération 9 de la boucle)

- `Cargo.toml` du workspace (les quatre crates suivent via `version.workspace`), `installer/ForgePlayer.iss` et le lien de téléchargement du `README.md` passent à **1.6.0**. Plus aucune occurrence de `1.5.0` hors journaux.
- `RELEASE_NOTES.md` : section v1.6.0 complète (corrections HDR, corrections 4K/2K, vérifications chiffrées).
- **Preuve d'exécution** : le binaire construit journalise `ForgePlayer v1.6.0` au démarrage et lit un fichier sans erreur.

### Packaging et publication v1.6.0 (2026-09-23, itération 10 de la boucle)

- `build.bat release x64` : exécutable Rust + les deux services Go + DLL FFmpeg + shaders assemblés dans `dist\`.
- Binaires signés avec le certificat Heiphaistos (`D:\Projet\outils\signer\signer.ps1`) : `ForgePlayer.exe`, `subtitle-service.exe`, `media-indexer.exe`, puis l'installateur. `Get-AuthenticodeSignature` rend `UnknownError` sur cette machine — normal, la racine auto-signée n'y est pas installée ; le signataire lu est bien `CN=Heiphaistos`.
- `ForgePlayer_v1.6.0_Portable.zip` (99 Mo) et `ForgePlayer_v1.6.0_Setup.exe` (67 Mo, Inno Setup) produits ; anciens artefacts 1.5.0 supprimés de `dist\`.
- **Test du binaire packagé** (pas du build de développement) : `dist\ForgePlayer.exe` journalise `ForgePlayer v1.6.0`, détecte `HDR: transfert=1`, affiche l'image correctement (capture à l'appui).
- Poussé sur GitHub : 9 commits (`ab32ded..4b2fa81`) et release **v1.6.0** publiée avec les deux artefacts — https://github.com/Heiphaistos/ForgePlayer/releases/tag/v1.6.0

### Lecture réseau HTTP/HLS et URL `file://` (2026-09-23, itération 11 de la boucle)

- [TESTED-OK] **HTTP** : 1080p H.264 et 4K HDR10 HEVC 10-bit lus depuis `http://127.0.0.1:8099/…`, cadence temps réel (`pos == wall`), tampon audio 2,4 à 3,6 s.
- [TESTED-OK] **HLS** : playlist `stream.m3u8` (segments TS de 2 s, 4K HDR) lue, badge HDR et résolution corrects.
- [FIXED] **Aucune option réseau n'était passée à libavformat.** Conséquences mesurées : une URL injoignable restait 10 s sur « Chargement… » avant l'erreur, et une coupure en cours de lecture terminait le flux définitivement (FFmpeg a `reconnect=0` par défaut). Ajout de `timeout=3 s` (connexion TCP), `rw_timeout=8 s` (lectures), `reconnect`, `reconnect_streamed`, `reconnect_delay_max=4`, `user_agent=ForgePlayer/<version>` — appliquées à la sonde ET à l'ouverture, les deux tentant la connexion.
- **Mesure** : URL injoignable signalée en **6 s** au lieu de 10 s, avec le message exact à l'écran. Coupure réseau provoquée pendant une lecture 4K (serveur tué 5 s) : le journal montre `Will reconnect at 127926316 in 0/1 second(s)` et **la lecture continue sans arrêt** (position 34,33 → 46,39 s, tampon audio 2,9 à 3,6 s).
- ⚠ **`reconnect_on_network_error` est un piège** : il fait aussi réessayer la connexion INITIALE en boucle, donc une URL injoignable n'échoue plus jamais (mesuré : aucune erreur après 16 s). Option volontairement non activée.
- ⚠ **Le serveur de test doit gérer les requêtes Range** : `python -m http.server` répond 200 au lieu de 206, ce qui corrompt la lecture d'un mp4 dont le `moov` est à la fin (`Invalid NAL unit size`, 7 904 paquets refusés, 0 image décodée). Ce n'était PAS un défaut du lecteur — serveur de test corrigé (`scratchpad/range_server.py`), le même fichier passe ensuite sans une seule erreur.
- [FIXED] **`file://` ne fonctionnait pas** (défaut connu depuis v1.4.5) : le dialogue d'URL acceptait le schéma mais libavformat le refuse sous Windows. Converti en chemin local (`file:///D:/x.mkv` → `D:/x.mkv`, `%20` décodé, UNC préservé), à l'ouverture donc aussi pour un argument de ligne de commande. Le filtre de `main.rs` rejetait par ailleurs toute URL non-`http` : il accepte désormais n'importe quel schéma.

### Sous-titres bitmap PGS/VOBSUB/DVB (2026-09-23, itération 12 de la boucle)

- [FIXED] **Les sous-titres image n'étaient pas affichés du tout** (limitation connue de longue date, « non supportés »). Les pistes PGS/HDMV, VOBSUB et DVB sont maintenant décodées, converties en RGBA et incrustées dans le rectangle vidéo à l'échelle de la source.
- Chaîne ajoutée : `SubtitleBitmap` (rectangle RGBA + position) dans `omni-core`, conversion PAL8 → RGBA depuis `AVSubtitleRect` (`data[0]` = index, `data[1]` = palette ARGB), nouvel événement `PipelineEvent::SubtitleBitmap`, cues gardés côté player (64 max, ce sont des pixels), textures egui reconstruites **uniquement au changement de cue**.
- **Piège du format** : un paquet PGS n'a pas de durée — l'image reste affichée jusqu'à un paquet d'**effacement** (composition sans rectangle). La durée par défaut d'une seconde, héritée du texte, faisait disparaître le sous-titre presque aussitôt. Les compositions vides sont donc transmises elles aussi et bornent le cue précédent ; la durée par défaut passe à 30 s pour une piste bitmap.
- **Fabrication de l'échantillon de test** : FFmpeg refuse de convertir du texte en bitmap (« Subtitle encoding currently only possible from text to text or bitmap to bitmap »), donc impossible de générer un PGS avec lui. Un générateur `.sup` a été écrit (segments PCS/WDS/PDS/ODS/END, image palettisée, RLE PGS) : `scratchpad/make_pgs.py`. Fichier muxé en MKV, `ffprobe` le reconnaît comme `hdmv_pgs_subtitle`.
- **Mesure** : sur `pgs_test.mkv`, touche `S` pour activer la piste — le sous-titre s'affiche (bande basse de l'image : **8,94 %** de pixels quasi blancs) puis disparaît au paquet d'effacement (**0,00 %**). Capture à l'appui : texte blanc cerné de noir, centré, à l'échelle de la vidéo.
- VOBSUB et DVB passent par le même décodeur et le même chemin d'affichage ; seul PGS a pu être vérifié ici, faute d'échantillon.

### AV1 4K HDR et activation automatique des sous-titres (2026-09-23, itération 13 de la boucle)

- [TESTED-OK] **AV1 4K HDR 10 bits** (`libsvtav1`, BT.2020/PQ, 3840×2160) : `HW accel initialisé: D3D11Va`, badge `AV1 · HDR · 4K UHD`, lecture temps réel (`pos == wall` à ±10 ms), tampon audio 2,1 à 3,2 s. Aucun défaut trouvé, rien à corriger.
- [FIXED] **Les sous-titres restaient éteints à l'ouverture** : il fallait presser `S`. VLC et mpv activent une piste automatiquement. Nouveau réglage `subtitle_auto` (activé par défaut, case à cocher dans Paramètres) : à l'ouverture, la première piste dont la langue correspond à `subtitle_lang` est activée, sinon la première piste du fichier.
- **Mesure** : sur `pgs_test.mkv`, sans aucune touche, le journal indique `sous-titres : piste 0 activée automatiquement (1 pistes)` et la bande basse de l'image contient **8,94 %** de pixels quasi blancs — exactement la valeur mesurée quand la piste était activée à la main.

### Capture d'image (2026-09-23, itération 14 de la boucle)

- [NOUVEAU] **Capture de l'image affichée** — fonction que VLC a depuis toujours et qui manquait ici. `Maj+S` ou menu *Vue → Capture d'image*. Le fichier va dans `Images\ForgePlayer\<titre>_<position>s.png`, à la **résolution source** (3840×2160 sur le fichier de test), et un OSD annonce le chemin.
- Implémentation par **relecture GPU** : les passes d'affichage sont rejouées vers une texture hors écran (`SNAPSHOT_FORMAT = Rgba8Unorm`), puis `copy_texture_to_buffer` + `map_async`. L'image enregistrée est donc exactement celle qui est vue — tone mapping HDR, matrice de couleur, plage et échelle 10 bits comprises — sans dupliquer ces calculs sur le processeur.
- ⚠ Format non-sRGB volontaire : le shader écrit déjà des valeurs encodées sRGB, une texture `...Srgb` les aurait encodées deux fois.
- **Mesure** : quantiles de luminance (p5/p25/p50/p75/p95) du PNG enregistré **0,132 / 0,250 / 0,538 / 0,853 / 1,000** contre **0,134 / 0,250 / 0,528 / 0,853 / 1,000** pour la fenêtre affichée — écart maximal 0,010, dû au ré-échantillonnage de la fenêtre (1238 px) vers la source (3840 px).
- ⚠ `pilote.py` n'envoie que des touches simples : le raccourci `Maj+S` n'est pas testable par ce canal, le test passe par le clic sur l'entrée de menu (qui appelle le même code).

### Reprise de lecture (2026-09-23, itération 15 de la boucle)

- [NOUVEAU] **Reprise là où on s'est arrêté**, comme VLC et mpv. La position est enregistrée dans la config (50 fichiers au maximum) à la fermeture propre et au changement de média, puis appliquée à la réouverture avec un OSD « Reprise à m:ss ». Réglage `resume_playback` (activé par défaut, case à cocher dans Paramètres).
- Les positions proches du début ou de la fin (moins de 30 s de chaque côté) ne sont pas mémorisées : reprendre à 3 s ou dans le générique n'a aucun intérêt.
- **Mesure** : lecture de `long_hdr_4k.mp4`, cinq sauts en avant, fermeture propre → `config.json` contient `("D:\Projet\ForgePlayer\.testmedia\long_hdr_4k.mp4", 55.755…)`. Réouverture : journal `reprise de lecture à 55.8 s`, et la barre affiche **1:15 / 5:30** quelques secondes plus tard (capture à l'appui) au lieu de repartir de zéro.

### Vitesse de lecture : le son suit enfin (2026-09-23, itération 16 de la boucle)

- [FIXED] **Régler la vitesse ne changeait rien quand le fichier avait du son.** `set_speed` ne touchait que l'horloge ; le son continuait à sortir à 1×, et comme c'est lui l'horloge de référence, la lecture restait à 1× — le badge affichait bien « 1,5× » mais rien n'accélérait. La limitation était même écrite dans `sync_clock_to_audio` (« l'audio jouera à 1×, désynchronisé »).
- Le son passe maintenant par le filtre `atempo` de libavfilter (**durée modifiée, hauteur conservée**, comme VLC et mpv), enchaîné automatiquement au-delà de 2× ou en dessous de 0,5×. Les frames produites sont réhorodatées en temps MÉDIA, donc l'audio reste l'horloge de référence et l'image reste calée dessus.
- Nouvelle commande de pipeline `SetSpeed`, entrées de menu *Lecture → Moins vite / Plus vite / Vitesse normale* (les raccourcis `[` et `]` existaient déjà).
- **Mesure** (fichier 4K HDR avec son) :
  - avant : à « 1,5× », `pos` avançait de 3,01 s pour 3,00 s de temps réel — soit **1,00×**, le réglage était inerte ;
  - après, à 1,5× : `pos` passe de 159,68 s à 181,75 s pendant que le temps réel passe de 105,37 s à 120,44 s, soit **1,465×** (la montée en régime du filtre explique l'écart au 1,5 théorique), `audio_master=true` conservé, tampon audio entre 2,7 et 3,5 s ;
  - retour à 1× : 3,00 s de média pour 3,01 s de temps réel.

### Fourmillement à la réduction d'échelle (2026-09-23, itération 17 de la boucle)

- [FIXED] **Une image 4K affichée dans une fenêtre trois fois plus petite fourmillait.** L'échantillonnage bilinéaire ne moyenne que 2×2 texels : sur une réduction 3×, les deux tiers des texels ne sont jamais lus. Mesuré sur un plan fixe 4K : **+31,6 %** d'énergie hautes fréquences (variance du laplacien) par rapport au même plan réduit en Lanczos — c'est de l'aliasing, pas du détail.
- Correctif : moyenne 3×3 sur l'empreinte réelle du pixel, obtenue par `fwidth` calculé en flux de contrôle uniforme puis passé aux `textureSampleLevel` (qui n'ont pas besoin de dérivées, donc l'appel reste légal hors flux uniforme).
- **Piège** : appliquer le filtre au seul shader YUV→RGB ne change RIEN pour le HDR (mesuré : 0,02236 → 0,02218). En HDR cette passe rend à la résolution de la SOURCE dans une texture hors écran ; c'est la passe de tone mapping qui réduit ensuite à la taille de la fenêtre. Le filtre doit donc exister dans les deux shaders.
- **Mesure finale** : énergie hautes fréquences 0,02236 → **0,01578**, soit **+31,6 % → −7,1 %** par rapport à la référence Lanczos (légèrement plus doux, ce qu'on attend d'une moyenne face à un sinc fenêtré).

### Passage en revue après les nouveautés + version 1.7.0 (2026-09-23, itération 18 de la boucle)

- [TESTED-OK] Relecture de six fichiers après les changements des itérations 11 à 17 : `bbb_1080p_sdr.mp4` (1080p SDR), `sdr8_2k.mp4` (2K), `sdr10_4k.mp4` (4K 10 bits SDR), `av1_4k_hdr.mp4` (4K AV1 HDR), `pgs_test.mkv` (sous-titres image), `still_full.mp4` (plage complète). Tous en temps réel (`pos == wall` à ±50 ms), **aucune ligne ERROR ni panique**. Les fichiers sans piste audio affichent logiquement `audio_master=false`.
- Version portée à **1.7.0** partout (`Cargo.toml` du workspace, `installer/ForgePlayer.iss`, lien du `README.md`), section v1.7.0 écrite dans `RELEASE_NOTES.md`. Binaire vérifié : journalise `ForgePlayer v1.7.0`.

### Packaging et publication v1.7.0 (2026-09-23, itération 19 de la boucle)

- `build.bat release x64`, binaires et installateur signés (certificat Heiphaistos), `ForgePlayer_v1.7.0_Portable.zip` (99,1 Mo) et `ForgePlayer_v1.7.0_Setup.exe` (67,4 Mo) produits, artefacts 1.6.0 retirés de `dist\`.
- **Test du binaire packagé** (pas du build de développement) sur `pgs_test.mkv` : journal `ForgePlayer v1.7.0` puis `sous-titres : piste 0 activée automatiquement`, et la capture montre le sous-titre image incrusté sans qu'aucune touche n'ait été pressée.
- Release publiée avec les deux artefacts : https://github.com/Heiphaistos/ForgePlayer/releases/tag/v1.7.0

### Repli 5.1/7.1 vers la stéréo : fin de l'écrêtage (2026-09-23, itération 20 de la boucle)

- [FIXED] **Le repli multicanal saturait.** La somme `FL + 0,707·FC + 0,707·BL` atteint **2,41** à pleine échelle et était simplement écrêtée à 1,0 : de la distorsion audible sur tout film 5.1 un peu fort, c'est-à-dire la quasi-totalité des films 4K. Le repli est maintenant **normalisé par le total des coefficients** (même méthode que le `rematrix` de FFmpeg et que VLC) : à pleine échelle la sortie vaut exactement 1,0, jamais plus.
- Les coefficients sont désormais déclarés par disposition (5.1 = FL FR FC LFE BL BR, 7.1 = + SL SR) au lieu d'être recopiés à la main dans deux branches. Le caisson (LFE) reste hors du mélange, comme chez FFmpeg.
- **6 tests unitaires** ajoutés (`cargo test -p omni-audio`, tous verts) : mono dupliqué, stéréo inchangée, 5.1 et 7.1 à pleine échelle qui donnent exactement 1,0 sans écrêtage, canal avant gauche seul qui laisse la droite muette (0,414 / 0,000), LFE ignoré, longueur de sortie.
- **Lecture réelle** d'un fichier 5.1 AC-3 448 kb/s (six tonalités distinctes) : périphérique ouvert en stéréo, `audio_master=true`, tampon 2,5 s, cadence temps réel, aucune erreur.

### Coût processeur mesuré face à VLC (2026-09-23, itération 21 de la boucle)

- **Comparaison sur le même fichier 4K HDR10** (330 s, D3D11VA) : ForgePlayer **80 % d'un cœur** (63,8 s de processeur pour 80 s de lecture), VLC **8 %** (6,3 s). Mémoire : 1,8 Go contre 1,5 Go.
- **Profil par image 4K** (sondes `DBGPERF`, `RUST_LOG=debug`) : rapatriement GPU→RAM **17,85 ms**, extraction des plans **5,36 ms**, envoi vers le GPU **2,42 ms** — soit ~25,6 ms par image à 24 img/s, ce qui explique la mesure.
- [FIXED] **La frame système de destination était réallouée à chaque image** : `av_hwframe_transfer_data` sur une frame vide alloue 24 Mo par image en 4K. Elle est désormais réutilisée d'une image à l'autre (sortie de `self` le temps du traitement pour ne pas bloquer l'emprunt du scaler). **Mesure : 80 % → 59 % d'un cœur**, rapatriement 17,85 → 15,42 ms, extraction 5,36 → 4,43 ms.
- **Limite restante, chiffrée** : les 15,4 ms qui restent sont la copie GPU→RAM elle-même, inhérente au mode « décodage matériel puis rapatriement ». VLC ne la paie pas : il garde la surface sur le GPU (zéro-copie D3D11). C'est le seul écart de performance encore ouvert, et le chemin pour le fermer est l'interopérabilité D3D11 ↔ wgpu, qui suppose de forcer le backend DX12 (l'application tourne actuellement sur Vulkan) et de passer par `wgpu_hal`.
- Compatibilité vérifiée au passage, sans anomalie : **VFR** (cadence variable), **ProRes 422 10 bits**, audio **44,1 kHz** (pas de dérive : `pos` suit `wall`), fichier 1918×1078.

### Reste à faire

- [ ] Utiliser les métadonnées de mastering réelles (MaxCLL / master-display) comme pic de tone mapping, au lieu de la valeur figée `max_luminance` de la config.
- [ ] Vérifier HLG sur un vrai fichier HLG (aucun disponible ici pour l'instant).
- [ ] Plage de couleur complète (full/JPEG range) : le shader YUV suppose toujours du limited range.
- [ ] 10-bit décodé en logiciel : la passe CPU `shift_10_to_16` subsiste (chemin de repli uniquement, le décodage matériel ne l'emprunte plus).
- [ ] Zéro-copie D3D11 ↔ wgpu (toujours « hw decode + copy-back »).


## Journal chronologique
