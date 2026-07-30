use std::sync::Arc;
use parking_lot::Mutex;
use omni_core::decoder::DecodedVideoFrame;
use omni_renderer::{HdrTonemapper, ToneMapParams, VideoRenderer, HDR_OFFSCREEN_FORMAT};

use eframe::{egui_wgpu, wgpu};

pub type SharedFrame = Arc<Mutex<Option<DecodedVideoFrame>>>;

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
    /// Vrai si le contenu actuellement affiché est HDR 10-bit — état persistant
    /// côté app (pas seulement "y a-t-il un nouveau frame ce tick"), car le
    /// chemin de rendu doit rester cohérent même sur les repaints sans
    /// nouvelle frame (pause, throttling).
    pub is_hdr:        bool,
    pub tonemap_mode:  u32,
    pub max_luminance: f32,
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
            renderer.set_color_space(queue, self.color_space);
            if let Some(frame) = self.frame.lock().take() {
                renderer.upload_frame(device, queue, &frame);
            }
        }

        if self.is_hdr {
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
                            _pad: 0.0,
                        });
                    }
                }
            }
        }

        vec![]
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        rp:   &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if self.is_hdr {
            if let Some(tonemapper) = resources.get::<HdrTonemapper>() {
                tonemapper.render(rp);
            }
        } else if let Some(renderer) = resources.get::<VideoRenderer>() {
            renderer.render(rp);
        }
    }
}
