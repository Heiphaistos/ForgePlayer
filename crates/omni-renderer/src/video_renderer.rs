use anyhow::Result;
use wgpu::{util::DeviceExt, *};

use crate::frame_upload::{TexLayout, YuvTextures};
use omni_core::decoder::DecodedVideoFrame;

const SHADER_SRC: &str = include_str!("../../../assets/shaders/yuv_to_rgb.wgsl");

/// Renderer wgpu : upload YUV → rendu RGB via shader WGSL.
pub struct VideoRenderer {
    pipeline:          RenderPipeline,
    /// Même shader, mais ciblant une texture offscreen Rgba16Float au lieu du
    /// swapchain — utilisé pour le contenu HDR : cette passe produit du RGB
    /// encodé PQ (pas encore de la lumière linéaire), qu'`HdrTonemapper`
    /// consomme ensuite pour le vrai tone mapping avant affichage SDR.
    pipeline_offscreen: RenderPipeline,
    sampler:           Sampler,
    bind_group_layout: BindGroupLayout,
    bind_group:        Option<BindGroup>,
    yuv_textures:      Option<YuvTextures>,
    uniform_buf:       Buffer,
    uniform_bg:        BindGroup,
    #[allow(dead_code)] uniform_bgl: BindGroupLayout,
    current_color_space: u32,  // 0=BT601, 1=BT709, 2=BT2020
    /// Disposition chroma du dernier frame uploadé — recopiée dans les
    /// uniforms (`offset.x`) pour que le shader sache s'il doit lire V dans
    /// une troisième texture (planaire) ou dans le canal G de la deuxième
    /// (semi-planaire NV12/P010).
    current_semi_planar: bool,
    /// Vrai si le flux courant est en plage complète (JPEG/PC).
    current_full_range: bool,
    /// Vrai si le device a accordé `TEXTURE_FORMAT_16BIT_NORM` — sinon le
    /// contenu HDR 10-bit est affiché en 8-bit (repli silencieux, pas pire
    /// qu'avant cette fonctionnalité, jamais un crash).
    supports_16bit: bool,
}

/// Format de la texture intermédiaire HDR (RGB encodé PQ, pas encore tonemap).
pub const HDR_OFFSCREEN_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// Uniforms envoyés au shader — layout colonne-major pour WGSL mat4x4.
/// `matrix[i]` = ième colonne. Vecteur input = [y', u', v', 1.0].
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ColorUniforms {
    matrix: [[f32; 4]; 4],
    offset: [f32; 4],
}

impl ColorUniforms {
    /// Construit la matrice YUV→RGB depuis les coefficients de luminance de
    /// l'espace colorimétrique et la plage des échantillons.
    ///
    /// Plage limitée (MPEG/TV) : Y ∈ [16,235], chroma ∈ [16,240] — il faut
    /// retirer l'offset 16 et ré-étendre (255/219 en luma, 255/224 en chroma).
    /// Plage complète (JPEG/PC) : aucun offset de luma, aucune extension.
    /// Appliquer les facteurs du limited range à du full range délave tout
    /// (noirs gris, blancs écrêtés) — et inversement l'image sort trop
    /// contrastée.
    fn from_coeffs(kr: f32, kb: f32, full_range: bool) -> Self {
        let kg = 1.0 - kr - kb;
        let (y_scale, c_scale, y_offset) = if full_range {
            (1.0, 1.0, 0.0)
        } else {
            (255.0 / 219.0, 255.0 / 224.0, 16.0 / 255.0)
        };

        let vr =  (2.0 - 2.0 * kr) * c_scale;
        let ub =  (2.0 - 2.0 * kb) * c_scale;
        let ug = -(kb / kg) * (2.0 - 2.0 * kb) * c_scale;
        let vg = -(kr / kg) * (2.0 - 2.0 * kr) * c_scale;

        Self {
            // Colonnes : [Y], [U], [V], [1]
            matrix: [
                [y_scale, y_scale, y_scale, 0.0],
                [0.0,     ug,      ub,      0.0],
                [vr,      vg,      0.0,     0.0],
                [0.0,     0.0,     0.0,     1.0],
            ],
            // offset.x = drapeau chroma semi-planaire, offset.y = offset de luma.
            offset: [0.0, y_offset, 0.0, 0.0],
        }
    }

    fn bt601(full: bool)  -> Self { Self::from_coeffs(0.299,  0.114,  full) }
    fn bt709(full: bool)  -> Self { Self::from_coeffs(0.2126, 0.0722, full) }
    fn bt2020(full: bool) -> Self { Self::from_coeffs(0.2627, 0.0593, full) }

    fn with_semi(mut self, semi: bool) -> Self {
        self.offset[0] = if semi { 1.0 } else { 0.0 };
        self
    }
}

impl VideoRenderer {
    pub fn new(device: &Device, surface_format: TextureFormat) -> Result<Self> {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label:  Some("yuv_to_rgb"),
            source: ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        let sampler = device.create_sampler(&SamplerDescriptor {
            label:        Some("yuv_sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter:   FilterMode::Linear,
            min_filter:   FilterMode::Linear,
            ..Default::default()
        });

        // BGL pour les 3 textures YUV + sampler
        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label:   Some("yuv_bgl"),
            entries: &[
                texture_entry(0), texture_entry(1), texture_entry(2),
                BindGroupLayoutEntry {
                    binding:    3,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // BGL uniforms couleur
        let uniform_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label:   Some("color_uniform_bgl"),
            entries: &[BindGroupLayoutEntry {
                binding:    0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty:                 BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size:   None,
                },
                count: None,
            }],
        });

        let uniforms    = ColorUniforms::bt709(false);
        let uniform_buf = device.create_buffer_init(&util::BufferInitDescriptor {
            label:    Some("color_uniform_buf"),
            contents: bytemuck::bytes_of(&uniforms),
            usage:    BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });

        let uniform_bg = device.create_bind_group(&BindGroupDescriptor {
            label:   Some("color_uniform_bg"),
            layout:  &uniform_bgl,
            entries: &[BindGroupEntry {
                binding:  0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label:                Some("video_pipeline_layout"),
            bind_group_layouts:   &[&bind_group_layout, &uniform_bgl],
            push_constant_ranges: &[],
        });

        let make_pipeline = |target_format: TextureFormat, label: &str| {
            device.create_render_pipeline(&RenderPipelineDescriptor {
                label:       Some(label),
                layout:      Some(&pipeline_layout),
                vertex:      VertexState {
                    module:      &shader,
                    entry_point: Some("vs_main"),
                    buffers:     &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(FragmentState {
                    module:      &shader,
                    entry_point: Some("fs_main"),
                    targets:     &[Some(ColorTargetState {
                        format:     target_format,
                        blend:      None,
                        write_mask: ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive:    PrimitiveState {
                    topology: PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample:   MultisampleState::default(),
                multiview:     None,
                cache:         None,
            })
        };

        let pipeline            = make_pipeline(surface_format, "video_pipeline");
        let pipeline_offscreen  = make_pipeline(HDR_OFFSCREEN_FORMAT, "video_pipeline_hdr_offscreen");

        Ok(Self {
            pipeline,
            pipeline_offscreen,
            sampler,
            bind_group_layout,
            bind_group: None,
            yuv_textures: None,
            uniform_buf,
            uniform_bg,
            uniform_bgl,
            current_color_space: 1,  // BT.709 par défaut
            current_semi_planar: false,
            current_full_range: false,
            supports_16bit: device.features().contains(Features::TEXTURE_FORMAT_16BIT_NORM),
        })
    }

    /// Met à jour l'espace colorimétrique (0=BT601, 1=BT709, 2=BT2020) et la
    /// plage des échantillons (limitée 16-235 ou complète 0-255).
    pub fn set_color_space(&mut self, queue: &Queue, cs: u32, full_range: bool) {
        if self.current_color_space == cs && self.current_full_range == full_range { return; }
        self.current_color_space = cs;
        self.current_full_range  = full_range;
        self.write_uniforms(queue);
    }

    fn write_uniforms(&self, queue: &Queue) {
        let full = self.current_full_range;
        let uniforms = match self.current_color_space {
            0 => ColorUniforms::bt601(full),
            2 => ColorUniforms::bt2020(full),
            _ => ColorUniforms::bt709(full),
        }
        .with_semi(self.current_semi_planar);
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Met à jour les textures avec un nouveau frame.
    pub fn upload_frame(&mut self, device: &Device, queue: &Queue, frame: &DecodedVideoFrame) {
        let semi = frame.format.is_semi_planar();
        if semi != self.current_semi_planar {
            self.current_semi_planar = semi;
            self.write_uniforms(queue);
        }

        let textures = YuvTextures::ensure(
            self.yuv_textures.take(),
            device,
            frame.width,
            frame.height,
            TexLayout { semi, bits16: frame.format.is_hdr10bit() && self.supports_16bit },
        );
        textures.upload(queue, frame);

        let yv = textures.y.create_view(&Default::default());
        let uv = textures.u.create_view(&Default::default());
        // Chroma entrelacée : la même texture RG sert aux deux slots, le shader
        // n'échantillonne alors que le slot U (canaux R et G).
        let vv = if semi { uv.clone() } else { textures.v.create_view(&Default::default()) };

        self.bind_group = Some(device.create_bind_group(&BindGroupDescriptor {
            label:   Some("yuv_bg"),
            layout:  &self.bind_group_layout,
            entries: &[
                BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&yv) },
                BindGroupEntry { binding: 1, resource: BindingResource::TextureView(&uv) },
                BindGroupEntry { binding: 2, resource: BindingResource::TextureView(&vv) },
                BindGroupEntry { binding: 3, resource: BindingResource::Sampler(&self.sampler) },
            ],
        }));

        self.yuv_textures = Some(textures);
    }

    /// Encode le pass de rendu vidéo dans un RenderPass existant.
    pub fn render(&self, rp: &mut RenderPass<'static>) {
        if let Some(bg) = &self.bind_group {
            rp.set_pipeline(&self.pipeline);
            rp.set_bind_group(0, bg, &[]);
            rp.set_bind_group(1, &self.uniform_bg, &[]);
            rp.draw(0..4, 0..1);
        }
    }

    /// Encode le pass YUV→RGB(PQ) vers une texture offscreen (chemin HDR) —
    /// ouvre et referme son propre RenderPass sur l'encoder donné, puisque
    /// `paint()` d'egui_wgpu ne fournit qu'un seul RenderPass déjà lié au
    /// swapchain (impossible d'y rediriger la sortie). Appelé depuis
    /// `prepare()`, avant le pass principal d'egui.
    pub fn render_to_offscreen(&self, encoder: &mut CommandEncoder, target: &TextureView) {
        let Some(bg) = &self.bind_group else { return };
        let mut rp = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("video_hdr_offscreen_pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Color::BLACK), store: StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        rp.set_pipeline(&self.pipeline_offscreen);
        rp.set_bind_group(0, bg, &[]);
        rp.set_bind_group(1, &self.uniform_bg, &[]);
        rp.draw(0..4, 0..1);
    }

    /// Dimensions du dernier frame uploadé, si disponible — utilisé pour
    /// dimensionner la texture offscreen HDR sans conserver le frame
    /// lui-même (l'upload le consomme).
    pub fn frame_size(&self) -> Option<(u32, u32)> {
        self.yuv_textures.as_ref().map(|t| (t.width, t.height))
    }
}

fn texture_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Texture {
            sample_type:    TextureSampleType::Float { filterable: true },
            view_dimension: TextureViewDimension::D2,
            multisampled:   false,
        },
        count: None,
    }
}
