use wgpu::{Device, Queue, Texture, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages};
use omni_core::decoder::{DecodedVideoFrame, PixelFormat};

/// Disposition des textures GPU pour un frame décodé.
///
/// `semi` = chroma entrelacée dans un seul plan (NV12/P010, sortie native du
/// décodage matériel) : la texture `u` est alors RG et `v` n'est qu'un
/// remplissage 1×1 pour satisfaire le bind group.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TexLayout {
    pub semi:  bool,
    pub bits16: bool,
}

/// Textures GPU pour un frame YUV planaire (8/10-bit) ou semi-planaire (NV12/P010).
pub struct YuvTextures {
    pub y:  Texture,
    pub u:  Texture,
    pub v:  Texture,
    pub width:  u32,
    pub height: u32,
    layout: TexLayout,
}

impl YuvTextures {
    /// Alloue ou ré-alloue les textures si la résolution ou la disposition change.
    pub fn ensure(
        current: Option<Self>,
        device:  &Device,
        w: u32,
        h: u32,
        layout: TexLayout,
    ) -> Self {
        if let Some(t) = current {
            if t.width == w && t.height == h && t.layout == layout { return t; }
        }
        let luma_fmt = if layout.bits16 { TextureFormat::R16Unorm } else { TextureFormat::R8Unorm };
        let chroma_fmt = match (layout.semi, layout.bits16) {
            (true, true)  => TextureFormat::Rg16Unorm,
            (true, false) => TextureFormat::Rg8Unorm,
            (false, _)    => luma_fmt,
        };
        let make = |fmt: TextureFormat, lw: u32, lh: u32| {
            device.create_texture(&TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: lw.max(1), height: lh.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count:    1,
                dimension:       TextureDimension::D2,
                format:          fmt,
                usage:           TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats:    &[],
            })
        };
        let (cw, ch) = (w / 2, h / 2);
        Self {
            y: make(luma_fmt, w, h),
            u: make(chroma_fmt, cw, ch),
            // Chroma entrelacée : `v` n'est jamais échantillonnée, on garde une
            // texture minimale du même format pour que le bind group reste valide.
            v: if layout.semi { make(chroma_fmt, 1, 1) } else { make(chroma_fmt, cw, ch) },
            width: w, height: h, layout,
        }
    }

    pub fn layout(&self) -> TexLayout { self.layout }

    /// Upload un frame décodé vers les textures GPU.
    pub fn upload(&self, queue: &Queue, frame: &DecodedVideoFrame) {
        let upload = |tex: &Texture, data: &[u8], stride: usize, w: u32, h: u32| {
            queue.write_texture(
                tex.as_image_copy(),
                data,
                wgpu::TexelCopyBufferLayout {
                    offset:         0,
                    bytes_per_row:  Some(stride as u32),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        };
        let (cw, ch) = (self.width / 2, self.height / 2);

        match frame.format {
            // Planaire 8-bit, ou planaire 10-bit quand le GPU accorde R16Unorm :
            // les plans partent tels quels.
            PixelFormat::Yuv420p => {
                upload(&self.y, &frame.planes[0], frame.strides[0], self.width, self.height);
                upload(&self.u, &frame.planes[1], frame.strides[1], cw, ch);
                upload(&self.v, &frame.planes[2], frame.strides[2], cw, ch);
            }
            PixelFormat::Yuv420p10le if self.layout.bits16 => {
                upload(&self.y, &frame.planes[0], frame.strides[0], self.width, self.height);
                upload(&self.u, &frame.planes[1], frame.strides[1], cw, ch);
                upload(&self.v, &frame.planes[2], frame.strides[2], cw, ch);
            }
            // Planaire 10-bit sur un GPU sans TEXTURE_FORMAT_16BIT_NORM :
            // on garde l'octet haut de chaque échantillon (÷256).
            PixelFormat::Yuv420p10le => {
                let y8 = narrow_16_to_8(&frame.planes[0], frame.strides[0], self.width as usize, self.height as usize, 1);
                let u8_ = narrow_16_to_8(&frame.planes[1], frame.strides[1], cw as usize, ch as usize, 1);
                let v8 = narrow_16_to_8(&frame.planes[2], frame.strides[2], cw as usize, ch as usize, 1);
                upload(&self.y, &y8, self.width as usize, self.width, self.height);
                upload(&self.u, &u8_, cw as usize, cw, ch);
                upload(&self.v, &v8, cw as usize, cw, ch);
            }
            // Semi-planaire 8-bit (NV12) : Y + UV entrelacé, aucune conversion.
            PixelFormat::Nv12 => {
                upload(&self.y, &frame.planes[0], frame.strides[0], self.width, self.height);
                upload(&self.u, &frame.planes[1], frame.strides[1], cw, ch);
            }
            // Semi-planaire 10-bit (P010) : idem si le GPU accorde Rg16Unorm,
            // sinon repli 8-bit sur l'octet haut (2 composantes par texel).
            PixelFormat::P010Le if self.layout.bits16 => {
                upload(&self.y, &frame.planes[0], frame.strides[0], self.width, self.height);
                upload(&self.u, &frame.planes[1], frame.strides[1], cw, ch);
            }
            PixelFormat::P010Le => {
                let y8 = narrow_16_to_8(&frame.planes[0], frame.strides[0], self.width as usize, self.height as usize, 1);
                let uv8 = narrow_16_to_8(&frame.planes[1], frame.strides[1], cw as usize, ch as usize, 2);
                upload(&self.y, &y8, self.width as usize, self.width, self.height);
                upload(&self.u, &uv8, cw as usize * 2, cw, ch);
            }
            _ => {
                log::warn!("format {:?} non géré dans upload YUV", frame.format);
            }
        }
    }
}

/// Réduit des échantillons 16-bit alignés sur les bits hauts (P010) en 8-bit
/// tassés, en respectant le stride source (padding de fin de ligne possible).
/// `comps` = nombre de composantes par texel (1 = plan luma/chroma planaire,
/// 2 = chroma entrelacée).
fn narrow_16_to_8(data: &[u8], src_stride: usize, w: usize, h: usize, comps: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * comps);
    for row in 0..h {
        let row_start = row * src_stride;
        for i in 0..w * comps {
            let idx = row_start + i * 2 + 1; // octet haut de l'échantillon 16-bit LE
            out.push(data.get(idx).copied().unwrap_or(0));
        }
    }
    out
}
