# ForgePlayer — Journal de débogage

Mis à jour en continu pendant la campagne de test sur PC réel (audio matériel).
Format par entrée : `[STATUT] Zone — description`. STATUT ∈ {FIXED, OPEN, TESTED-OK, TODO}.

---

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

## Journal chronologique
