mod app;
mod config;
mod player;
mod playlist_io;
mod services;
mod ui;
mod video_callback;

use anyhow::Result;
use eframe::{NativeOptions, egui::ViewportBuilder, egui_wgpu, wgpu};
use std::sync::Arc;

fn main() -> Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .init();

    log::info!("OmniPlayer v{}", env!("CARGO_PKG_VERSION"));

    // Charge la config utilisateur
    let cfg = config::AppConfig::load();

    // Fichier passé en argument (association de fichiers / « Ouvrir avec » / CLI)
    let initial_file = std::env::args().nth(1)
        .filter(|p| p.starts_with("http") || std::path::Path::new(p).exists());

    let options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("OmniPlayer")
            .with_inner_size([cfg.window_width as f32, cfg.window_height as f32])
            .with_min_inner_size([640.0, 400.0])
            .with_icon(load_icon()),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew {
                device_descriptor: Arc::new(|adapter| {
                    // 16-bit non-normalisé : nécessaire pour l'upload direct des
                    // textures YUV 10-bit HDR sans passer par une texture entière
                    // + normalisation manuelle dans le shader. Demandé seulement
                    // si l'adaptateur le supporte réellement (repli silencieux en
                    // 8-bit sur le matériel qui ne l'a pas — cf. VideoRenderer).
                    let optional = wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
                    let base_limits = if adapter.get_info().backend == wgpu::Backend::Gl {
                        wgpu::Limits::downlevel_webgl2_defaults()
                    } else {
                        wgpu::Limits::default()
                    };
                    wgpu::DeviceDescriptor {
                        label: Some("omniplayer wgpu device"),
                        required_features: adapter.features() & optional,
                        required_limits: wgpu::Limits { max_texture_dimension_2d: 8192, ..base_limits },
                        memory_hints: wgpu::MemoryHints::default(),
                    }
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        "OmniPlayer",
        options,
        Box::new(|cc| Ok(Box::new(app::OmniApp::new(cc, cfg, initial_file)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))
}

fn load_icon() -> Arc<egui::IconData> {
    // Génère procéduralement une icône 32×32 dégradé bleu (#0080FF) → violet (#8000FF).
    // Fond circulaire sombre + dégradé horizontal sur les pixels intérieurs.
    const SIZE: u32 = 32;
    let cx = SIZE as f32 / 2.0;
    let cy = SIZE as f32 / 2.0;
    let r = cx - 1.0;

    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist <= r {
                // Dégradé horizontal bleu → violet
                let t = x as f32 / (SIZE - 1) as f32;
                let red   = (t * 128.0) as u8;
                let green = 0u8;
                let blue  = 255u8;
                // Légère vignette sur les bords du cercle
                let alpha = if dist > r - 1.5 {
                    ((r - dist) * 170.0).clamp(0.0, 255.0) as u8
                } else {
                    255u8
                };
                rgba.push(red);
                rgba.push(green);
                rgba.push(blue);
                rgba.push(alpha);
            } else {
                // Hors du cercle → transparent
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    Arc::new(egui::IconData { rgba, width: SIZE, height: SIZE })
}
