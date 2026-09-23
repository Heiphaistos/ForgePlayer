//! Affiche une texture RGB déjà prête, sans conversion.
//!
//! Utilisé par le chemin sans copie : la conversion YUV→RGB a été faite par le
//! processeur vidéo du GPU, directement dans une texture partagée.

use wgpu::*;

const SHADER: &str = include_str!("../../../assets/shaders/rgb_passthrough.wgsl");

pub struct RgbPassthrough {
    pipeline:   RenderPipeline,
    layout:     BindGroupLayout,
    sampler:    Sampler,
    bind_group: Option<BindGroup>,
}

impl RgbPassthrough {
    pub fn new(device: &Device, out_format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label:  Some("rgb_passthrough"),
            source: ShaderSource::Wgsl(SHADER.into()),
        });
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("rgb_passthrough_sampler"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("rgb_passthrough_bgl"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("rgb_passthrough_layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("rgb_passthrough_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState {
                    format: out_format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        Self { pipeline, layout, sampler, bind_group: None }
    }

    pub fn set_input(&mut self, device: &Device, view: &TextureView) {
        self.bind_group = Some(device.create_bind_group(&BindGroupDescriptor {
            label: Some("rgb_passthrough_bg"),
            layout: &self.layout,
            entries: &[
                BindGroupEntry { binding: 0, resource: BindingResource::TextureView(view) },
                BindGroupEntry { binding: 1, resource: BindingResource::Sampler(&self.sampler) },
            ],
        }));
    }

    pub fn render(&self, rp: &mut RenderPass<'static>) {
        if let Some(bg) = &self.bind_group {
            rp.set_pipeline(&self.pipeline);
            rp.set_bind_group(0, bg, &[]);
            rp.draw(0..4, 0..1);
        }
    }
}
