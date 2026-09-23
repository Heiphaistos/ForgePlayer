//! Partage des images décodées entre D3D11 (FFmpeg) et le rendu wgpu, sans
//! passer par la mémoire centrale.
//!
//! Le décodage matériel produit une texture D3D11. Jusqu'ici elle était
//! rapatriée en RAM (`av_hwframe_transfer_data`), recopiée dans des `Vec`, puis
//! ré-envoyée au GPU : trois traversées de bus par image, ~25 ms de processeur
//! par image 4K. Ici, rien ne bouge : un processeur vidéo D3D11 convertit la
//! surface YUV en RGB directement dans une texture **partagée** allouée côté
//! D3D12, que wgpu échantillonne telle quelle.
//!
//! Sens du partage : la ressource est créée en D3D12 (c'est wgpu qui doit
//! pouvoir la lire) puis ouverte en D3D11, et non l'inverse — D3D12 ne sait pas
//! prendre un mutex à clé, qui est la seule façon de partager dans l'autre sens.
//! La synchronisation passe par une barrière partagée : D3D11 la signale après
//! sa conversion, la file D3D12 l'attend avant de dessiner.

#![cfg(windows)]

use anyhow::{bail, Context as _, Result};
use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, HANDLE, GENERIC_ALL};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

/// Nombre de textures partagées en rotation. Le rendu peut lire l'image
/// précédente pendant que le décodeur écrit la suivante ; trois suffisent avec
/// la file de six images du pipeline.
const SLOTS: usize = 3;

/// Format de la texture partagée : 10 bits par composante, ce que réclame le
/// HDR. Le HDR10 arrive en PQ et repart en PQ — la conversion de dynamique
/// reste faite par notre shader, le processeur vidéo ne fait que la matrice
/// YUV→RGB.
const SHARED_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R10G10B10A2_UNORM;

/// Appareil D3D11 créé sur le MÊME adaptateur que le rendu.
///
/// Sans cette précaution, FFmpeg ouvre son propre appareil sur l'adaptateur par
/// défaut — l'Intel intégré sur un portable hybride — alors que wgpu rend sur
/// la carte NVIDIA. Le partage échoue alors avec « paramètre incorrect » :
/// deux GPU différents ne partagent ni texture ni barrière.
pub struct SharedD3d11Device {
    device: ID3D11Device,
}

// Le pointeur est confié à FFmpeg, qui l'utilise depuis son thread de
// décodage ; l'appareil est créé en mode multithread protégé pour cela.
unsafe impl Send for SharedD3d11Device {}
unsafe impl Sync for SharedD3d11Device {}

impl SharedD3d11Device {
    pub fn as_ptr(&self) -> *mut std::ffi::c_void {
        unsafe { std::mem::transmute_copy(&self.device) }
    }
}

/// Crée un appareil D3D11 sur l'adaptateur utilisé par wgpu.
pub fn create_d3d11_on_render_adapter(device: &wgpu::Device) -> Result<SharedD3d11Device> {
    let d3d12_device = unsafe {
        device.as_hal::<wgpu::hal::api::Dx12, _, _>(|hal| hal.map(|d| d.raw_device().clone()))
    }
    .context("le rendu ne tourne pas sur DX12")?;

    let luid = unsafe { d3d12_device.GetAdapterLuid() };
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.context("CreateDXGIFactory1")?;

    let mut chosen: Option<IDXGIAdapter1> = None;
    let mut i = 0u32;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(i) } {
        let desc = unsafe { adapter.GetDesc1() }.context("GetDesc1")?;
        if desc.AdapterLuid.LowPart == luid.LowPart && desc.AdapterLuid.HighPart == luid.HighPart {
            chosen = Some(adapter);
            break;
        }
        i += 1;
    }
    let adapter = chosen.context("adaptateur du rendu introuvable côté DXGI")?;

    let mut d3d11: Option<ID3D11Device> = None;
    let mut ctx: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            None,
            // Support vidéo : indispensable au décodage matériel et au
            // processeur vidéo. BGRA : exigé par certains pilotes pour les
            // ressources partagées.
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut d3d11),
            None,
            Some(&mut ctx),
        )
    }
    .context("D3D11CreateDevice sur l'adaptateur du rendu")?;

    let d3d11 = d3d11.context("appareil D3D11 nul")?;
    let ctx = ctx.context("contexte D3D11 nul")?;

    // FFmpeg décode depuis son propre thread pendant que le rendu convertit :
    // sans cette protection, les deux se marchent dessus.
    let mt: ID3D11Multithread = ctx.cast().context("ID3D11Multithread indisponible")?;
    unsafe { mt.SetMultithreadProtected(true) };

    log::info!("zéro-copie : appareil D3D11 créé sur l'adaptateur du rendu");
    Ok(SharedD3d11Device { device: d3d11 })
}

pub struct SharedSlot {
    pub texture: wgpu::Texture,
    pub view:    wgpu::TextureView,
    d3d11:       ID3D11Texture2D,
    #[allow(dead_code)]
    d3d12:       ID3D12Resource,
}

pub struct ZeroCopyBridge {
    slots:        Vec<SharedSlot>,
    next_slot:    usize,
    fence12:      ID3D12Fence,
    fence11:      ID3D11Fence,
    fence_value:  u64,
    queue12:      ID3D12CommandQueue,
    d3d11_ctx:    ID3D11DeviceContext4,
    video_ctx:    ID3D11VideoContext1,
    video_device: ID3D11VideoDevice,
    processor:    ID3D11VideoProcessor,
    enumerator:   ID3D11VideoProcessorEnumerator,
    width:        u32,
    height:       u32,
    /// Appareil D3D11 du décodeur : si un autre fichier ouvre un nouvel
    /// appareil, le pont doit être reconstruit.
    device_ptr:   *mut std::ffi::c_void,
}

// Le type-map d'egui exige `Send + Sync`. Les objets COM ici ne sont
// manipulés que depuis le thread de rendu (préparation et peinture du callback
// s'exécutent sur ce seul thread), et l'appareil D3D11 du décodeur est protégé
// par le verrou de FFmpeg pour les accès concurrents du décodage.
unsafe impl Send for ZeroCopyBridge {}
unsafe impl Sync for ZeroCopyBridge {}

impl ZeroCopyBridge {
    /// Construit le pont pour une résolution donnée. `d3d11_device` vient du
    /// décodeur FFmpeg ; `device`/`queue` sont ceux de wgpu, qui doit tourner
    /// sur le backend DX12.
    pub fn new(
        device:       &wgpu::Device,
        d3d11_device: *mut std::ffi::c_void,
        width:        u32,
        height:       u32,
    ) -> Result<Self> {
        if d3d11_device.is_null() {
            bail!("appareil D3D11 du décodeur inconnu");
        }

        // ── Côté wgpu : on récupère l'appareil et la file D3D12 bruts ───────
        let (d3d12_device, queue12) = unsafe {
            device.as_hal::<wgpu::hal::api::Dx12, _, _>(|hal| {
                hal.map(|d| (d.raw_device().clone(), d.raw_queue().clone()))
            })
        }
        .context("le rendu ne tourne pas sur DX12 (pas de partage possible)")?;

        let d3d11: ID3D11Device = unsafe {
            ID3D11Device::from_raw_borrowed(&d3d11_device)
                .context("appareil D3D11 invalide")?
                .clone()
        };
        let d3d11_1: ID3D11Device1 = d3d11.cast().context("ID3D11Device1 indisponible")?;
        let d3d11_5: ID3D11Device5 = d3d11.cast()
            .context("ID3D11Device5 indisponible (Windows trop ancien pour les barrières partagées)")?;

        let d3d11_ctx: ID3D11DeviceContext4 = unsafe { d3d11.GetImmediateContext() }
            .context("contexte immédiat D3D11 absent")?
            .cast()
            .context("ID3D11DeviceContext4 indisponible")?;

        let video_device: ID3D11VideoDevice = d3d11.cast().context("ID3D11VideoDevice indisponible")?;
        let video_ctx: ID3D11VideoContext1 = d3d11_ctx.cast().context("ID3D11VideoContext1 indisponible")?;

        // ── Processeur vidéo : conversion YUV → RGB sur le GPU ──────────────
        let content_desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputWidth:  width,
            InputHeight: height,
            OutputWidth:  width,
            OutputHeight: height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            ..Default::default()
        };
        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&content_desc) }
            .context("CreateVideoProcessorEnumerator")?;
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }
            .context("CreateVideoProcessor")?;

        // ── Barrière partagée D3D12 → D3D11 ─────────────────────────────────
        let fence12: ID3D12Fence = unsafe { d3d12_device.CreateFence(0, D3D12_FENCE_FLAG_SHARED) }
            .context("CreateFence partagée")?;
        let fence_handle = unsafe {
            d3d12_device.CreateSharedHandle(&fence12, None, GENERIC_ALL.0, None)
        }
        .context("CreateSharedHandle (barrière)")?;
        let mut fence11: Option<ID3D11Fence> = None;
        unsafe { d3d11_5.OpenSharedFence(fence_handle, &mut fence11) }
            .context("OpenSharedFence côté D3D11")?;
        let fence11 = fence11.context("barrière partagée nulle côté D3D11")?;
        unsafe { CloseHandle(fence_handle).ok() };

        // ── Textures partagées ──────────────────────────────────────────────
        let mut slots = Vec::with_capacity(SLOTS);
        for i in 0..SLOTS {
            slots.push(Self::make_slot(device, &d3d12_device, &d3d11_1, width, height, i)?);
        }

        log::info!(
            "zéro-copie : pont D3D11 ↔ DX12 actif ({width}×{height}, {SLOTS} surfaces partagées)"
        );

        Ok(Self {
            slots,
            next_slot: 0,
            fence12,
            fence11,
            fence_value: 0,
            queue12,
            d3d11_ctx,
            video_ctx,
            video_device,
            processor,
            enumerator,
            width,
            height,
            device_ptr: d3d11_device,
        })
    }

    fn make_slot(
        device:       &wgpu::Device,
        d3d12_device: &ID3D12Device,
        d3d11_1:      &ID3D11Device1,
        width:  u32,
        height: u32,
        index:  usize,
    ) -> Result<SharedSlot> {
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width:  width as u64,
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: SHARED_FORMAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            // Le processeur vidéo D3D11 écrit dedans via une vue de sortie :
            // il lui faut le droit « cible de rendu ».
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        };
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };

        let mut res: Option<ID3D12Resource> = None;
        unsafe {
            d3d12_device.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_SHARED,
                &desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut res,
            )
        }
        .context("CreateCommittedResource partagée")?;
        let res = res.context("ressource partagée nulle")?;

        let handle: HANDLE = unsafe {
            d3d12_device.CreateSharedHandle(&res, None, GENERIC_ALL.0, None)
        }
        .context("CreateSharedHandle (texture)")?;
        let d3d11_tex: ID3D11Texture2D = unsafe { d3d11_1.OpenSharedResource1(handle) }
            .context("OpenSharedResource1 côté D3D11")?;
        unsafe { CloseHandle(handle).ok() };

        let hal_tex = unsafe {
            wgpu::hal::dx12::Device::texture_from_raw(
                res.clone(),
                wgpu::TextureFormat::Rgb10a2Unorm,
                wgpu::TextureDimension::D2,
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                1,
                1,
            )
        };
        let texture = unsafe {
            device.create_texture_from_hal::<wgpu::hal::api::Dx12>(
                hal_tex,
                &wgpu::TextureDescriptor {
                    label: Some(&format!("surface_partagee_{index}")),
                    size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgb10a2Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let view = texture.create_view(&Default::default());

        Ok(SharedSlot { texture, view, d3d11: d3d11_tex, d3d12: res })
    }

    pub fn matches(&self, device_ptr: *mut std::ffi::c_void, width: u32, height: u32) -> bool {
        self.device_ptr == device_ptr && self.width == width && self.height == height
    }

    /// Convertit une surface décodée dans la prochaine texture partagée et
    /// renvoie la vue à échantillonner. Aucun octet ne passe par le processeur.
    pub fn convert(
        &mut self,
        decoded_texture: *mut std::ffi::c_void,
        array_index: u32,
        hdr: bool,
        full_range: bool,
    ) -> Result<&wgpu::TextureView> {
        let input: ID3D11Texture2D = unsafe {
            ID3D11Texture2D::from_raw_borrowed(&decoded_texture)
                .context("texture décodée invalide")?
                .clone()
        };

        let slot_index = self.next_slot;
        self.next_slot = (self.next_slot + 1) % SLOTS;

        let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: array_index },
            },
        };
        let mut in_view: Option<ID3D11VideoProcessorInputView> = None;
        unsafe {
            self.video_device.CreateVideoProcessorInputView(
                &input, &self.enumerator, &in_desc, Some(&mut in_view),
            )
        }
        .context("CreateVideoProcessorInputView")?;
        let in_view = in_view.context("vue d'entrée nulle")?;

        let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut out_view: Option<ID3D11VideoProcessorOutputView> = None;
        unsafe {
            self.video_device.CreateVideoProcessorOutputView(
                &self.slots[slot_index].d3d11, &self.enumerator, &out_desc, Some(&mut out_view),
            )
        }
        .context("CreateVideoProcessorOutputView")?;
        let out_view = out_view.context("vue de sortie nulle")?;

        // Espaces colorimétriques : on garde la fonction de transfert de la
        // source (PQ en HDR10). Le processeur vidéo ne fait que la matrice
        // YUV→RGB ; la conversion de dynamique reste dans notre shader.
        let (in_space, out_space) = if hdr {
            (
                DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020,
                DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020,
            )
        } else if full_range {
            (
                DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709,
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            )
        } else {
            (
                DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            )
        };
        unsafe {
            self.video_ctx.VideoProcessorSetStreamColorSpace1(&self.processor, 0, in_space);
            self.video_ctx.VideoProcessorSetOutputColorSpace1(&self.processor, out_space);
        }

        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            pInputSurface: unsafe { std::mem::transmute_copy(&in_view) },
            ..Default::default()
        };
        unsafe {
            self.video_ctx.VideoProcessorBlt(&self.processor, &out_view, 0, &[stream])
                .context("VideoProcessorBlt")?;
        }

        // La file D3D12 doit attendre la fin de cette conversion avant de lire
        // la texture : sans cette barrière, le rendu peut échantillonner une
        // image à moitié écrite.
        self.fence_value += 1;
        unsafe {
            self.d3d11_ctx.Signal(&self.fence11, self.fence_value).context("Signal D3D11")?;
            self.d3d11_ctx.Flush();
            self.queue12.Wait(&self.fence12, self.fence_value).context("Wait D3D12")?;
        }

        Ok(&self.slots[slot_index].view)
    }
}
