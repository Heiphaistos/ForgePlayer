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
    /// Planar YUV 4:2:0 10-bit (HDR10/HLG). Échantillons 16-bit, décalés de 6
    /// bits vers la gauche (convention P010) : la valeur 10-bit d'origine
    /// occupe les bits hauts, ce qui permet au shader existant (pensé pour
    /// des textures normalisées 8-bit) de fonctionner sans changement — le
    /// ratio noir/blanc/plage limitée est identique en 8 et 10 bits.
    Yuv420p10le,
    /// Semi-planaire 10-bit (Y 16-bit + UV entrelacé 16-bit, valeurs alignées
    /// sur les bits hauts) — sortie native du décodage matériel 10-bit,
    /// envoyée au GPU sans conversion.
    P010Le,
    Rgba,        // fallback RGBA
}

impl PixelFormat {
    /// Vrai si les échantillons sont stockés sur 16 bits (source 10-bit).
    pub fn is_hdr10bit(self) -> bool {
        matches!(self, PixelFormat::Yuv420p10le | PixelFormat::P010Le)
    }

    /// Vrai si la chroma est entrelacée dans un seul plan (NV12 / P010).
    pub fn is_semi_planar(self) -> bool {
        matches!(self, PixelFormat::Nv12 | PixelFormat::P010Le)
    }
}
