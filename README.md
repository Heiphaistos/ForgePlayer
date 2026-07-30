<div align="center">
  <h1>ForgePlayer</h1>
  <p><strong>Lecteur multimédia haute performance sous Windows — pipeline Rust/wgpu + services Go.</strong></p>

  ![Version](https://img.shields.io/badge/version-1.4.5-blue)
  ![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-0078D4?logo=windows)
  ![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange?logo=rust)
  ![Go](https://img.shields.io/badge/Go-1.22-00ADD8?logo=go)
  ![License](https://img.shields.io/badge/licence-MIT-green)
</div>

---

## Description

ForgePlayer est un lecteur multimédia natif Windows construit sur un pipeline Rust entièrement multithreadé. Il utilise FFmpeg 7.x pour le décodage universel, wgpu/DirectX 12 pour le rendu GPU avec shaders WGSL (conversion YUV→RGB), et CPAL pour la sortie audio. Deux services Go légers assurent la recherche de sous-titres via OpenSubtitles et l'indexation de bibliothèque locale avec métadonnées TMDB.

---

## Fonctionnalités

- **Décodage universel** — FFmpeg 7.x : H.264, H.265, AV1, VP9, MPEG-2, MPEG-4 et 30+ formats
- **Rendu GPU natif** — pipeline wgpu/DirectX 12 avec shader WGSL YUV→RGB, espaces colorimétriques BT.601/709/2020
- **HDR** — détection PQ (SMPTE 2084) et HLG (ARIB STD-B67) avec badge dans l'interface
- **Accélération matérielle** — DXVA2, D3D11VA, NVDEC, AMF, QuickSync (sélection automatique)
- **Sous-titres** — SRT, ASS, SSA, VTT + recherche automatique via OpenSubtitles
- **Lecture réseau** — HTTP, RTMP, HLS (`.m3u8`), DASH (`.mpd`)
- **Visionneuse d'images** — JPEG, PNG, WebP, HEIC, AVIF, RAW (CR2, NEF, ARW…) et 30+ formats photo
- **Multi-pistes** — changement piste audio/sous-titre à la volée (touches `A` / `S`)
- **Chapitres** — navigation et marqueurs visuels sur la seekbar
- **Playlist** — glisser-déposer, boucle Off/×1/Tout, navigation clavier
- **Vitesse variable** — 0.25× à 4×
- **Volume jusqu'à 150%** — amplification logicielle
- **Overlay diagnostics** (touche `I`) — codec, résolution, FPS, buffer, HDR
- **Bibliothèque médias** — indexation de dossiers locaux, enrichissement TMDB
- **Distribution ZIP portable** — 92.6 MB, aucune installation requise

---

## Stack technique

| Couche | Technologies |
|--------|-------------|
| Lecteur principal | Rust 2021 + egui 0.31 + eframe |
| Rendu GPU | wgpu 24 (DirectX 12 / Vulkan) + WGSL |
| Décodage audio/vidéo | FFmpeg 7.x (ffmpeg-next 8) |
| Audio output | CPAL 0.15 + rubato 0.15 (rééchantillonnage) |
| Services annexes | Go 1.22 (sous-titres OpenSubtitles + indexeur TMDB) |
| Communication inter-threads | crossbeam-channel 0.5 + ringbuf 0.3 |

---

## Installation

### Option recommandée — Portable ZIP

1. Télécharger la dernière archive `ForgePlayer_v1.5.0_Portable.zip` depuis la [page Releases](https://github.com/Heiphaistos/ForgePlayer/releases).
2. Extraire dans le dossier de votre choix.
3. Lancer `launch.bat` (démarre les services Go + le lecteur).

> **Clés API optionnelles** (sous-titres et métadonnées TMDB) :
> ```bat
> set OPENSUBTITLES_API_KEY=votre_cle
> set TMDB_API_KEY=votre_cle
> ```

### Build depuis les sources

**Prérequis :**

| Outil | Version | Rôle |
|-------|---------|------|
| Rust + Cargo | 1.75+ stable MSVC | Compilateur principal |
| Go | 1.22+ | Services sous-titres / indexeur |
| FFmpeg | 7.x shared libs | Décodage audio/vidéo |
| Visual Studio Build Tools | 2022 | Toolchain MSVC |

```bat
REM Installation automatique des dépendances
setup.bat

REM Build release x64 (recommandé)
build.bat

REM Options
build.bat debug        # Mode debug
build.bat x32          # Architecture 32 bits
build.bat skip-go      # Ignorer la compilation Go
```

Les binaires sont générés dans `dist\` : `ForgePlayer.exe`, `subtitle-service.exe`, `media-indexer.exe` et les DLLs FFmpeg.

---

## Raccourcis clavier

| Touche | Action |
|--------|--------|
| `Espace` | Lecture / Pause |
| `← / →` | Reculer / Avancer de 10 s |
| `Shift+← / →` | Reculer / Avancer de 60 s |
| `Alt+← / →` | Reculer / Avancer de 1 s |
| `↑ / ↓` | Volume +10% / −10% |
| `M` | Muet |
| `A / S` | Piste audio / sous-titre suivante |
| `[ / ]` | Vitesse −/+ |
| `W` | Fit / Fill / Stretch |
| `L` | Mode boucle (Off / ×1 / Tout) |
| `F` | Plein écran |
| `I` | Overlay diagnostics |
| `Ctrl+O` | Ouvrir un fichier |
| `Ctrl+L` | Ouvrir une URL réseau |

---

## Architecture

```
ForgePlayer.exe (Rust)
├── omni-core      — Démultiplexage FFmpeg, décodage A/V, sync horloge
├── omni-renderer  — Rendu wgpu/DX12, shader WGSL YUV→RGB
├── omni-audio     — CPAL output, rubato resampler, ring buffer lock-free
└── omni-player    — UI egui, orchestration, playlist, config JSON

Services Go (HTTP 127.0.0.1 uniquement)
├── subtitle-service :18080  — OpenSubtitles v3 + métadonnées TMDB
└── media-indexer   :18081  — Scan bibliothèque locale + recherche textuelle
```

---

## Aperçu

> Captures disponibles lors de la première release publique.

---

## Licence

MIT — © 2026 Heiphaistos
