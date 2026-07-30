use std::env;
use std::fs;
use std::path::PathBuf;

#[cfg(windows)]
fn embed_windows_resources() {
    // .ico + métadonnées exe (FileDescription/ProductName/LegalCopyright déjà déclarées
    // dans [package.metadata.winresource] du Cargo.toml, mais jamais réellement
    // consommées faute d'appel ici -- l'exe tournait avec l'icône générique par défaut.
    winresource::WindowsResource::new()
        .set_icon("assets/icon.ico")
        .set("FileDescription", "OmniPlayer — Lecteur Multimédia Ultra-Complet")
        .set("ProductName", "OmniPlayer")
        .set("LegalCopyright", "MIT")
        .set("OriginalFilename", "omniplayer.exe")
        .compile()
        .expect("échec de l'intégration des ressources Windows (icône/métadonnées)");
}

fn main() {
    #[cfg(windows)]
    embed_windows_resources();

    let ffmpeg_dir = env::var("FFMPEG_DIR").unwrap_or_else(|_| "C:/ffmpeg".to_string());
    let ffmpeg_bin = PathBuf::from(&ffmpeg_dir).join("bin");

    // OUT_DIR = target/{profile}/build/omni-player-xxx/out
    // We want  target/{profile}/
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("unexpected OUT_DIR depth")
        .to_path_buf();

    if ffmpeg_bin.exists() {
        for entry in fs::read_dir(&ffmpeg_bin).expect("cannot read ffmpeg/bin") {
            let entry = entry.unwrap();
            let path  = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("dll") {
                let dest = target_dir.join(path.file_name().unwrap());
                if !dest.exists() {
                    fs::copy(&path, &dest).unwrap_or_else(|e| {
                        eprintln!("cargo:warning=Failed to copy {:?}: {e}", path.file_name().unwrap());
                        0
                    });
                }
            }
        }
        println!("cargo:rerun-if-changed={}", ffmpeg_bin.display());
    } else {
        println!("cargo:warning=FFMPEG_DIR/bin not found at {}", ffmpeg_bin.display());
    }
}
