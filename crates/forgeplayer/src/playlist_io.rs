use anyhow::{Context as _, Result};
use std::path::Path;

/// Écrit une playlist M3U8 (une entrée par ligne, chemins absolus, en-tête
/// `#EXTM3U` + `#EXTINF` pour le nom affiché par les autres lecteurs).
pub fn save_m3u(items: &[String], path: &Path) -> Result<()> {
    let mut out = String::from("#EXTM3U\n");
    for item in items {
        let name = Path::new(item)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| item.clone());
        out.push_str(&format!("#EXTINF:-1,{name}\n{item}\n"));
    }
    std::fs::write(path, out).with_context(|| format!("écriture playlist {}", path.display()))
}

/// Charge une playlist M3U/M3U8 — ignore les lignes `#` et vides, ne garde
/// que les chemins existants sur disque (une playlist déplacée depuis une
/// autre machine ne doit pas planter le lecteur, juste sauter les entrées
/// introuvables).
pub fn load_m3u(path: &Path) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("lecture playlist {}", path.display()))?;
    let base_dir = path.parent();

    let items = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            // Supporte les URL réseau telles quelles, et les chemins relatifs
            // résolus par rapport à l'emplacement du fichier .m3u.
            if l.starts_with("http://") || l.starts_with("https://") || l.starts_with("rtsp://") {
                return Some(l.to_string());
            }
            let p = std::path::Path::new(l);
            if p.is_absolute() {
                if p.exists() { Some(l.to_string()) } else { None }
            } else if let Some(dir) = base_dir {
                let joined = dir.join(p);
                if joined.exists() { Some(joined.to_string_lossy().to_string()) } else { None }
            } else {
                None
            }
        })
        .collect();

    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_absolute_paths() {
        let dir = std::env::temp_dir().join(format!("omni_playlist_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let media_a = dir.join("a.mp4");
        let media_b = dir.join("b sub folder needs quotes.mkv");
        std::fs::write(&media_a, b"x").unwrap();
        std::fs::write(&media_b, b"x").unwrap();

        let items = vec![
            media_a.to_string_lossy().to_string(),
            media_b.to_string_lossy().to_string(),
            "https://example.com/stream.m3u8".to_string(),
        ];

        let m3u_path = dir.join("playlist.m3u8");
        save_m3u(&items, &m3u_path).unwrap();
        let loaded = load_m3u(&m3u_path).unwrap();
        assert_eq!(loaded, items);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_skips_missing_files_and_comments() {
        let dir = std::env::temp_dir().join(format!("omni_playlist_test2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("exists.mp4");
        std::fs::write(&existing, b"x").unwrap();

        let m3u_path = dir.join("mixed.m3u8");
        std::fs::write(&m3u_path, format!(
            "#EXTM3U\n#EXTINF:-1,Ghost\n{}\n\n#EXTINF:-1,Real\n{}\n",
            dir.join("missing.mp4").display(),
            existing.display(),
        )).unwrap();

        let loaded = load_m3u(&m3u_path).unwrap();
        assert_eq!(loaded, vec![existing.to_string_lossy().to_string()]);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_resolves_relative_to_playlist_dir() {
        let dir = std::env::temp_dir().join(format!("omni_playlist_test3_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let media = dir.join("track.mp3");
        std::fs::write(&media, b"x").unwrap();

        let m3u_path = dir.join("rel.m3u");
        std::fs::write(&m3u_path, "track.mp3\n").unwrap();

        let loaded = load_m3u(&m3u_path).unwrap();
        assert_eq!(loaded, vec![media.to_string_lossy().to_string()]);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
