use std::sync::Arc;
use parking_lot::Mutex;
use omni_core::decoder::DecodedVideoFrame;
use omni_renderer::{HdrTonemapper, ToneMapParams, VideoRenderer, HDR_OFFSCREEN_FORMAT, SNAPSHOT_FORMAT};

/// Tone mapper dédié à la capture d'image : même traitement que l'affichage,
/// mais vers une texture 8 bits. Le type-map d'egui est indexé par type, d'où
/// ce nouveau type plutôt qu'un deuxième `HdrTonemapper`.
pub struct SnapshotTonemapper(pub HdrTonemapper);

use eframe::{egui_wgpu, wgpu};

pub type SharedFrame = Arc<Mutex<Option<DecodedVideoFrame>>>;

/// Demande et résultat d'une capture d'image, partagés entre l'UI et le
/// callback de rendu : l'image rendue n'existe que sur le GPU, donc seule la
/// passe de rendu peut la relire.
#[derive(Default)]
pub struct SnapshotState {
    pub requested: std::sync::atomic::AtomicBool,
    /// (largeur, hauteur, pixels RGBA) de la dernière capture.
    pub result:    Mutex<Option<(u32, u32, Vec<u8>)>>,
}

pub type SharedSnapshot = Arc<SnapshotState>;

/// Texture intermédiaire pour le chemin HDR (RGB encodé PQ avant tone mapping).
/// Recréée seulement quand la résolution change — pas à chaque frame.
#[derive(Default)]
pub struct HdrOffscreen {
    view: Option<wgpu::TextureView>,
    w:    u32,
    h:    u32,
}

impl HdrOffscreen {
    fn ensure(&mut self, device: &wgpu::Device, w: u32, h: u32) -> &wgpu::TextureView {
        if self.view.is_none() || self.w != w || self.h != h {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("hdr_offscreen"),
                size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count:    1,
                dimension:       wgpu::TextureDimension::D2,
                format:          HDR_OFFSCREEN_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats:    &[],
            });
            self.view = Some(tex.create_view(&Default::default()));
            self.w = w;
            self.h = h;
        }
        self.view.as_ref().unwrap()
    }
}

/// Callback egui_wgpu : upload la dernière frame YUV vers le GPU et encode le rendu.
/// Deux chemins : SDR direct (`VideoRenderer` → swapchain), ou HDR deux passes
/// (`VideoRenderer` → offscreen PQ-RGB, puis `HdrTonemapper` → swapchain).
pub struct VideoPaintCallback {
    pub frame:         SharedFrame,
    pub color_space:   u32,   // 0=BT601, 1=BT709, 2=BT2020
    /// Plage des échantillons : complète (JPEG/PC) ou limitée (MPEG/TV).
    pub full_range:    bool,
    /// Fonction de transfert du flux : 0 = SDR (rendu direct), 1 = PQ,
    /// 2 = HLG (rendu en deux passes avec tone mapping). Vient des métadonnées
    /// du flux, jamais de la profondeur de bits — un flux 10-bit BT.709 est du
    /// SDR et ressortirait brûlé s'il passait par le tone mapping.
    pub transfer:      u32,
    pub tonemap_mode:  u32,
    pub max_luminance: f32,
    /// Capture d'image demandée par l'utilisateur (touche Maj+S).
    pub snapshot:      SharedSnapshot,
}

impl VideoPaintCallback {
    /// Rejoue les passes d'affichage vers une texture hors écran puis relit ses
    /// pixels. Renvoie les octets RGBA, lignes compactées.
    fn capture(
        &self,
        device: &wgpu::Device,
        queue:  &wgpu::Queue,
        resources: &mut egui_wgpu::CallbackResources,
        w: u32,
        h: u32,
    ) -> Option<Vec<u8>> {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("snapshot"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count:    1,
            dimension:       wgpu::TextureDimension::D2,
            format:          SNAPSHOT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats:    &[],
        });
        let view = tex.create_view(&Default::default());

        // `copy_texture_to_buffer` impose des lignes alignées sur 256 octets :
        // la capture est donc relue avec du remplissage, retiré plus bas.
        let unpadded = w as usize * 4;
        let padded = unpadded.div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("snapshot_readback"),
            size:  (padded * h as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("snapshot_encoder"),
        });

        if self.transfer != 0 {
            // Chemin HDR : YUV → RGB encodé PQ hors écran, puis tone mapping
            // vers la texture de capture. Exactement ce que voit l'écran.
            let hdr_view = resources.get_mut::<HdrOffscreen>()
                .map(|off| off.ensure(device, w, h).clone())?;
            resources.get::<VideoRenderer>()?.render_to_offscreen(&mut enc, &hdr_view);
            let tm = resources.get_mut::<SnapshotTonemapper>()?;
            tm.0.set_input_texture(device, &hdr_view);
            tm.0.update_params(queue, &ToneMapParams {
                mode: self.tonemap_mode,
                max_luminance: self.max_luminance.max(1.0),
                exposure: 1.0,
                transfer: self.transfer,
            });
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("snapshot_tonemap_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            tm.0.render(&mut rp.forget_lifetime());
        } else {
            resources.get::<VideoRenderer>()?.render_to_snapshot(&mut enc, &view);
        }

        enc.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded as u32),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit(Some(enc.finish()));

        // Lecture synchrone : une capture est ponctuelle, quelques millisecondes
        // d'attente valent mieux qu'une machine à états sur plusieurs frames.
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::Maintain::Wait);

        let data = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity(unpadded * h as usize);
        for row in 0..h as usize {
            let start = row * padded;
            pixels.extend_from_slice(&data[start..start + unpadded]);
        }
        drop(data);
        buffer.unmap();
        Some(pixels)
    }
}

impl egui_wgpu::CallbackTrait for VideoPaintCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue:  &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        enc:    &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(renderer) = resources.get_mut::<VideoRenderer>() {
            renderer.set_color_space(queue, self.color_space, self.full_range);
            if let Some(frame) = self.frame.lock().take() {
                renderer.upload_frame(device, queue, &frame);
            }
        }

        if self.transfer != 0 {
            // Chemin HDR : passe 1 (YUV→RGB PQ vers texture offscreen) encodée
            // ici, car paint() ne reçoit qu'un seul RenderPass déjà lié au
            // swapchain — impossible d'y rediriger la sortie de cette passe.
            let size = resources.get::<VideoRenderer>().and_then(|r| r.frame_size());
            if let Some((w, h)) = size {
                // wgpu::TextureView est un handle bon marché à cloner (Arc en
                // interne) — on le sort de son emprunt sur `resources` tout de
                // suite pour ne pas bloquer les emprunts suivants sur d'autres
                // types du type-map (VideoRenderer, HdrTonemapper).
                let view = resources
                    .get_mut::<HdrOffscreen>()
                    .map(|off| off.ensure(device, w, h).clone());

                if let Some(view) = view {
                    if let Some(renderer) = resources.get::<VideoRenderer>() {
                        renderer.render_to_offscreen(enc, &view);
                    }
                    if let Some(tonemapper) = resources.get_mut::<HdrTonemapper>() {
                        tonemapper.set_input_texture(device, &view);
                        tonemapper.update_params(queue, &ToneMapParams {
                            mode: self.tonemap_mode,
                            max_luminance: self.max_luminance.max(1.0),
                            exposure: 1.0,
                            transfer: self.transfer,
                        });
                    }
                }
            }
        }

        // Capture : on refait exactement les passes d'affichage vers une texture
        // hors écran, puis on relit cette texture. Passer par le GPU garantit
        // que l'image enregistrée est celle qui est vue — tone mapping HDR,
        // matrice de couleur et plage comprises — sans refaire ces calculs sur
        // le processeur.
        if self.snapshot.requested.swap(false, std::sync::atomic::Ordering::Relaxed) {
            if let Some((w, h)) = resources.get::<VideoRenderer>().and_then(|r| r.frame_size()) {
                match self.capture(device, queue, resources, w, h) {
                    Some(pixels) => *self.snapshot.result.lock() = Some((w, h, pixels)),
                    None => log::warn!("capture d'image : lecture GPU impossible"),
                }
            }
        }

        vec![]
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        rp:   &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if self.transfer != 0 {
            if let Some(tonemapper) = resources.get::<HdrTonemapper>() {
                tonemapper.render(rp);
            }
        } else if let Some(renderer) = resources.get::<VideoRenderer>() {
            renderer.render(rp);
        }
    }
}
