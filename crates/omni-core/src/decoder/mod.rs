pub mod audio;
pub mod context;
pub mod subtitle;
pub mod video;

pub use audio::DecodedAudioFrame;
pub use video::DecodedVideoFrame;

/// Format pixel brut livré au renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Yuv420p,     // planar YUV 4:2:0 8-bit — le plus courant
    Yuv422p,     // planar YUV 4:2:2
    Yuv444p,     // planar YUV 4:4:4
    Nv12,        // semi-planar NV12 (HW accel output)
    /// Planar YUV 4:2:0 10-bit (sortie du décodage logiciel). Échantillons
    /// 16 bits avec la valeur 10 bits dans les bits BAS, tels que FFmpeg les
    /// produit : la remise à l'échelle est faite par le shader.
    Yuv420p10le,
    /// Semi-planaire 10-bit (Y 16-bit + UV entrelacé 16-bit, valeurs alignées
    /// sur les bits hauts) — sortie native du décodage matériel 10-bit,
    /// envoyée au GPU sans conversion.
    P010Le,
    Rgba,        // fallback RGBA
    /// Image restée sur le GPU : `DecodedVideoFrame::hw_surface()` donne la
    /// texture D3D11 et son indice dans le tableau. Aucun octet n'a transité
    /// par la mémoire centrale.
    D3d11,
}

impl PixelFormat {
    /// Vrai si les échantillons sont stockés sur 16 bits (source 10-bit).
    pub fn is_hdr10bit(self) -> bool {
        matches!(self, PixelFormat::Yuv420p10le | PixelFormat::P010Le)
    }

    /// Facteur à appliquer aux échantillons lus depuis la texture pour
    /// retomber sur une valeur normalisée 0..1. Le 10-bit planaire de FFmpeg
    /// est aligné sur les bits BAS (valeur/1023 stockée dans un mot 16 bits),
    /// le P010 du décodage matériel sur les bits HAUTS (déjà normalisé).
    pub fn sample_scale(self) -> f32 {
        match self {
            PixelFormat::Yuv420p10le => 65535.0 / 1023.0,
            _ => 1.0,
        }
    }

    /// Vrai si l'image n'existe que sur le GPU.
    pub fn is_gpu_surface(self) -> bool {
        matches!(self, PixelFormat::D3d11)
    }

    /// Vrai si la chroma est entrelacée dans un seul plan (NV12 / P010).
    pub fn is_semi_planar(self) -> bool {
        matches!(self, PixelFormat::Nv12 | PixelFormat::P010Le)
    }
}
