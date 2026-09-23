pub mod decoder;
pub mod hw_accel;
pub mod pipeline;
pub mod cpu_probe;
pub mod probe;

pub use decoder::{DecodedAudioFrame, DecodedVideoFrame};
pub use pipeline::{MediaPipeline, PipelineCommand, PipelineEvent};

/// Formats vidéo/audio/image supportés (FFmpeg gère tous les codecs).
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    // ── VIDEO CONTAINERS ──────────────────────────────────────────────────
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "ts", "m2ts",
    "mts", "mpg", "mpeg", "m4v", "3gp", "3g2", "ogv", "rm", "rmvb",
    "divx", "xvid", "vob", "ifo", "f4v", "asf", "mxf", "dv", "m2v",
    "h264", "h265", "hevc", "264", "265", "avc", "vc1",
    "av1", "ivf", "nuv", "nsv", "roq", "drc",
    // ── AUDIO FORMATS ─────────────────────────────────────────────────────
    "mp3", "aac", "flac", "ogg", "opus", "wav", "wma", "m4a", "ape",
    "mka", "mpa", "ac3", "eac3", "dts", "dtshd", "mlp", "truehd",
    "mp2", "mp1", "wv", "tta", "aiff", "aif", "au", "snd",
    "caf", "spx", "mpc", "ra", "amr", "gsm", "voc",
    // ── STREAMING ─────────────────────────────────────────────────────────
    "m3u8", "mpd", "m3u",
    // ── IMAGES ────────────────────────────────────────────────────────────
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif",
    "ico", "pnm", "pbm", "pgm", "ppm", "tga", "hdr",
    "avif", "heic", "heif", "jxl", "qoi",
    // RAW photo formats
    "raw", "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2",
    "pef", "srw", "x3f", "raf", "nrw",
];

/// Extensions image statique uniquement.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif",
    "ico", "pnm", "pbm", "pgm", "ppm", "tga", "hdr", "avif",
    "heic", "heif", "jxl", "qoi", "raw", "cr2", "cr3", "nef",
    "arw", "dng", "orf", "rw2", "pef", "srw", "x3f", "raf", "nrw",
];

/// Retourne `true` si le chemin correspond à une image statique.
pub fn is_image_path(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    IMAGE_EXTENSIONS.contains(&ext.as_str())
}

/// Codec hardware supporté détecté au runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HwAccelKind {
    None,
    Dxva2,
    D3D11Va,
    NvDec,
    AmfDec,
    QuickSync,
    Vulkan,
}

/// Résolution vidéo avec classification qualité.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    pub width:  u32,
    pub height: u32,
}

impl Resolution {
    pub fn quality_label(&self) -> &'static str {
        match (self.width, self.height) {
            (w, h) if w >= 7680 || h >= 4320 => "8K",
            (w, h) if w >= 3840 || h >= 2160 => "4K UHD",
            (w, h) if w >= 2560 || h >= 1440 => "1440p",
            (w, h) if w >= 1920 || h >= 1080 => "1080p",
            (w, h) if w >= 1280 || h >= 720  => "720p",
            (w, h) if w >= 854  || h >= 480  => "480p",
            _                                 => "SD",
        }
    }
}
