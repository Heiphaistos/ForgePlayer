use wgpu::{util::DeviceExt, *};

const HDR_SHADER: &str = include_str!("../../../assets/shaders/hdr_tonemap.wgsl");

/// Pipeline de tone-mapping HDR.
/// Prend en entrée une texture linéaire HDR (R16G16B16A16_FLOAT)
/// et sort une texture SDR (Bgra8UnormSrgb) pour l'affichage.
pub struct HdrTonemapper {
    pipeline:     RenderPipeline,
    sampler:      Sampler,
    bgl:          BindGroupLayout,
    uniform_buf:  Buffer,
    uniform_bg:   BindGroup,
    #[allow(dead_code)] uniform_bgl:  BindGroupLayout,
    bind_group:   Option<BindGroup>,
    #[allow(dead_code)] hdr_tex_size: Option<(u32, u32)>,
}

/// Paramètres uniforms envoyés au shader HDR.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ToneMapParams {
    pub mode:          u32,
    /// Pic de luminance du contenu, en nits.
    pub max_luminance: f32,
    pub exposure:      f32,
    /// Fonction de transfert de la source : 1 = PQ (SMPTE ST.2084),
    /// 2 = HLG (ARIB STD-B67). Le shader applique l'EOTF correspondante —
    /// appliquer la mauvaise courbe écrase l'image (trop sombre ou brûlée).
    pub transfer:      u32,
}

impl ToneMapParams {
    pub fn default_hdr() -> Self {
        Self { mode: 1, max_luminance: 1000.0, exposure: 1.0, transfer: 1 }
    }
}

impl HdrTonemapper {
    pub fn new(device: &Device, out_format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label:  Some("hdr_tonemap"),
            source: ShaderSource::Wgsl(HDR_SHADER.into()),
        });

        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("hdr_sampler"),
            ..Default::default()
        });

        let bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("hdr_bgl"),
            entries: &[
                BindGroupLayoutEntry {
                    binding:    0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type:    TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding:    1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let uniform_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label:   Some("hdr_uniform_bgl"),
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

        let params      = ToneMapParams::default_hdr();
        let uniform_buf = device.create_buffer_init(&util::BufferInitDescriptor {
            label:    Some("hdr_uniform_buf"),
            contents: bytemuck::bytes_of(&params),
            usage:    BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });

        let uniform_bg = device.create_bind_group(&BindGroupDescriptor {
            label:   Some("hdr_uniform_bg"),
            layout:  &uniform_bgl,
            entries: &[BindGroupEntry {
                binding:  0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label:                Some("hdr_pipeline_layout"),
            bind_group_layouts:   &[&bgl, &uniform_bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label:   Some("hdr_pipeline"),
            layout:  Some(&layout),
            vertex: VertexState {
                module:      &shader,
                entry_point: Some("vs_main"),
                buffers:     &[],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module:      &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState {
                    format:     out_format,
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
        });

        Self {
            pipeline,
            sampler,
            bgl,
            uniform_buf,
            uniform_bg,
            uniform_bgl,
            bind_group: None,
            hdr_tex_size: None,
        }
    }

    /// Met à jour les paramètres de tone mapping (mode, luminance, exposition).
    pub fn update_params(&self, queue: &Queue, params: &ToneMapParams) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(params));
    }

    /// Branche une texture HDR source sur le pipeline.
    pub fn set_input_texture(&mut self, device: &Device, tex_view: &TextureView) {
        self.bind_group = Some(device.create_bind_group(&BindGroupDescriptor {
            label:   Some("hdr_bg"),
            layout:  &self.bgl,
            entries: &[
                BindGroupEntry { binding: 0, resource: BindingResource::TextureView(tex_view) },
                BindGroupEntry { binding: 1, resource: BindingResource::Sampler(&self.sampler) },
            ],
        }));
    }

    /// Encode le pass de tone mapping dans un RenderPass existant.
    pub fn render(&self, rp: &mut RenderPass<'static>) {
        if let Some(bg) = &self.bind_group {
            rp.set_pipeline(&self.pipeline);
            rp.set_bind_group(0, bg, &[]);
            rp.set_bind_group(1, &self.uniform_bg, &[]);
            rp.draw(0..4, 0..1);
        }
    }
}
