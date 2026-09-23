//! Accélération matérielle — DXVA2, D3D11VA (Windows).
//!
//! Décode réellement sur le GPU : crée un `AVHWDeviceContext`, l'attache au
//! codec via `hw_device_ctx` + un callback `get_format` qui choisit le pixel
//! format hwaccel dans la liste proposée par le décodeur — le motif standard
//! documenté par FFmpeg (`hw_decode.c` dans ses exemples officiels).
//!
//! AVANT cette version, `apply_to_codec` ne faisait qu'activer le threading
//! logiciel : aucun hw_device_ctx n'était jamais attaché, donc TOUT flux
//! (y compris 4K HEVC/AV1 10-bit) décodait 100% en logiciel quel que soit le
//! réglage "d3d11va"/"dxva2" affiché dans les Paramètres — placebo. Le CPU
//! saturé par ce décodage logiciel lourd affamait le thread de démultiplexion
//! partagé (audio + lecture de paquets), d'où coupures audio et images en
//! retard sur les fichiers 4K/HDR.
//!
//! Les frames décodées restent sur le GPU (`AV_PIX_FMT_D3D11`/`DXVA2_VLD`) ;
//! `decoder/video.rs` les rapatrie en mémoire système via
//! `av_hwframe_transfer_data` avant de les faire suivre au pipeline logiciel
//! existant (extract_planes/SwsContext), inchangé. Repli logiciel automatique
//! et silencieux si l'initialisation ou la négociation du format échoue à
//! n'importe quelle étape — ne doit jamais faire planter la lecture.

#[cfg(windows)]
pub mod windows;

use ffmpeg_next as ffmpeg;
use ffmpeg_next::ffi;
use std::ptr;

/// Incrémente le compteur de références COM d'un pointeur brut.
#[cfg(windows)]
fn windows_add_ref(ptr: *mut std::ffi::c_void) {
    // Disposition COM : le premier pointeur de l'objet est sa table de
    // méthodes, dont la deuxième entrée est AddRef.
    unsafe {
        type AddRefFn = unsafe extern "system" fn(*mut std::ffi::c_void) -> u32;
        let vtable = *(ptr as *const *const *const std::ffi::c_void);
        let add_ref: AddRefFn = std::mem::transmute(*vtable.add(1));
        add_ref(ptr);
    }
}

pub struct HwAccelContext {
    pub kind:   HwKind,
    device_ref: *mut ffi::AVBufferRef,
}

// AVBufferRef référence un AVHWDeviceContext — aucun état thread-local, sûr à
// posséder depuis n'importe quel thread tant qu'il n'est pas utilisé de façon
// concurrente (chaque DecodeContext vit sur un seul thread demuxer à la fois).
unsafe impl Send for HwAccelContext {}

impl Drop for HwAccelContext {
    fn drop(&mut self) {
        if !self.device_ref.is_null() {
            unsafe { ffi::av_buffer_unref(&mut self.device_ref) };
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwKind {
    Dxva2,
    D3D11Va,
    Cuda,
    None,
}

/// Début de `AVD3D11VADeviceContext` (hwcontext_d3d11va.h). Seul le premier
/// champ nous intéresse : l'appareil D3D11 que FFmpeg doit utiliser au lieu
/// d'en ouvrir un à lui.
#[repr(C)]
struct AvD3d11VaDeviceContextHead {
    device:         *mut std::ffi::c_void,
    device_context: *mut std::ffi::c_void,
}

impl HwAccelContext {
    /// Construit un contexte D3D11VA autour d'un appareil DÉJÀ créé, celui du
    /// rendu. C'est la condition du partage sans copie : une texture ne se
    /// partage qu'entre appareils du même adaptateur.
    #[cfg(windows)]
    pub fn from_existing_d3d11(device: *mut std::ffi::c_void) -> anyhow::Result<Self> {
        if device.is_null() { anyhow::bail!("appareil D3D11 nul"); }
        unsafe {
            let device_ref = ffi::av_hwdevice_ctx_alloc(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA);
            if device_ref.is_null() { anyhow::bail!("av_hwdevice_ctx_alloc a échoué"); }

            let dctx = (*device_ref).data as *mut ffi::AVHWDeviceContext;
            let hwctx = (*dctx).hwctx as *mut AvD3d11VaDeviceContextHead;
            // FFmpeg libère cette référence à la destruction du contexte : on
            // lui en donne une à lui (AddRef) et on garde la nôtre.
            windows_add_ref(device);
            (*hwctx).device = device;

            let ret = ffi::av_hwdevice_ctx_init(device_ref);
            if ret < 0 {
                ffi::av_buffer_unref(&mut (device_ref as *mut _));
                anyhow::bail!("av_hwdevice_ctx_init (appareil fourni) a échoué (code {ret})");
            }
            log::info!("HW accel initialisé: D3D11Va (appareil du rendu, partage sans copie)");
            Ok(Self { kind: HwKind::D3D11Va, device_ref })
        }
    }

    /// Tente d'initialiser l'accélérateur nommé. Noms acceptés: "dxva2",
    /// "d3d11va" — tout le reste (dont "none"/inconnu) retourne un contexte
    /// `HwKind::None` inoffensif, jamais d'erreur. Seul un VRAI échec de
    /// création du device GPU (pilote absent, GPU incompatible) est remonté
    /// en erreur, pour laisser l'appelant retomber sur `None` proprement.
    pub fn try_init(name: &str) -> anyhow::Result<Self> {
        let (kind, av_type) = match name {
            "d3d11va" if cfg!(windows) => {
                (HwKind::D3D11Va, ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA)
            }
            "dxva2" if cfg!(windows) => {
                (HwKind::Dxva2, ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2)
            }
            _ => return Ok(Self { kind: HwKind::None, device_ref: ptr::null_mut() }),
        };

        let mut device_ref: *mut ffi::AVBufferRef = ptr::null_mut();
        let ret = unsafe {
            ffi::av_hwdevice_ctx_create(&mut device_ref, av_type, ptr::null(), ptr::null_mut(), 0)
        };
        if ret < 0 || device_ref.is_null() {
            anyhow::bail!("av_hwdevice_ctx_create({name}) a échoué (code {ret})");
        }
        log::info!("HW accel initialisé: {kind:?}");
        Ok(Self { kind, device_ref })
    }

    pub fn kind(&self) -> HwKind { self.kind }

    /// Applique le contexte HW au codec avant ouverture (avcodec_open2) —
    /// attache le device et le callback get_format. Doit être appelé AVANT
    /// `.decoder().video()` (context.rs respecte cet ordre).
    pub fn apply_to_codec(&self, ctx: &mut ffmpeg::codec::context::Context) {
        if matches!(self.kind, HwKind::Dxva2 | HwKind::D3D11Va | HwKind::Cuda) {
            // Threading frame-parallèle : profite toujours du multi-cœur, même
            // en repli logiciel si la négociation hwaccel échoue plus bas.
            ctx.set_threading(ffmpeg::codec::threading::Config {
                kind:  ffmpeg::codec::threading::Type::Frame,
                count: num_cpus(),
                // `..Default::default()` plutôt qu'un littéral complet : le champ
                // `safe` n'existe que pour FFmpeg < 6.0 (cfg côté patch ffmpeg-next),
                // absent sur la build Windows (FFmpeg 8.x via BtbN) mais requis sur
                // Linux (FFmpeg 4.4.2 via apt Ubuntu 22.04) — portable dans les deux cas.
                ..Default::default()
            });
        }

        if self.device_ref.is_null() { return; }
        let get_format = match self.kind {
            HwKind::D3D11Va => get_format_d3d11va,
            HwKind::Dxva2   => get_format_dxva2,
            _               => return,
        };
        unsafe {
            let raw = ctx.as_mut_ptr();
            (*raw).hw_device_ctx = ffi::av_buffer_ref(self.device_ref);
            (*raw).get_format    = Some(get_format);
        }
    }
}

fn num_cpus() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8)
}

unsafe extern "C" fn get_format_d3d11va(
    _ctx: *mut ffi::AVCodecContext,
    fmts: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    unsafe { select_hw_format(fmts, ffi::AVPixelFormat::AV_PIX_FMT_D3D11) }
}

unsafe extern "C" fn get_format_dxva2(
    _ctx: *mut ffi::AVCodecContext,
    fmts: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    unsafe { select_hw_format(fmts, ffi::AVPixelFormat::AV_PIX_FMT_DXVA2_VLD) }
}

/// Cherche `want` dans la liste `fmts` (terminée par AV_PIX_FMT_NONE) proposée
/// par le décodeur pour ce flux. Absent (GPU/pilote/codec ne supporte pas ce
/// hwaccel ici) → repli logiciel : renvoie le premier format de la liste, ce
/// qui fait décoder FFmpeg normalement en logiciel sans jamais planter.
unsafe fn select_hw_format(
    fmts: *const ffi::AVPixelFormat,
    want: ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    let mut p = fmts;
    unsafe {
        while *p != ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if *p == want { return want; }
            p = p.add(1);
        }
        *fmts
    }
}
