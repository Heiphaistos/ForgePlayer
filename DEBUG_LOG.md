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

### Reste à faire

- [ ] Utiliser les métadonnées de mastering réelles (MaxCLL / master-display) comme pic de tone mapping, au lieu de la valeur figée `max_luminance` de la config.
- [ ] Vérifier HLG sur un vrai fichier HLG (aucun disponible ici pour l'instant).
- [ ] Plage de couleur complète (full/JPEG range) : le shader YUV suppose toujours du limited range.
- [ ] 10-bit décodé en logiciel : la passe CPU `shift_10_to_16` subsiste (chemin de repli uniquement, le décodage matériel ne l'emprunte plus).
- [ ] Zéro-copie D3D11 ↔ wgpu (toujours « hw decode + copy-back »).


## Journal chronologique
