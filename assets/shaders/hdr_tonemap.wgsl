// ─── HDR Tone Mapping (PQ / HLG → SDR) ──────────────────────────────────────
// Post-process appliqué à la texture RGB encodée (PQ ou HLG) produite par la
// première passe YUV→RGB. Chaîne identique à celle de VLC/libplacebo :
//   signal encodé → lumière linéaire (nits) → normalisation blanc diffus
//   → tone mapping → conversion de gamut BT.2020 → BT.709 → encodage sRGB.

@group(0) @binding(0) var hdr_tex: texture_2d<f32>;
@group(0) @binding(1) var samp:    sampler;

struct ToneMapParams {
    mode:          u32,   // 0=Reinhard étendu, 1=ACES, 2=Hable
    max_luminance: f32,   // pic de luminance du contenu, en nits (ex: 1000)
    exposure:      f32,
    transfer:      u32,   // 1=PQ (SMPTE ST.2084), 2=HLG (ARIB STD-B67)
};
@group(1) @binding(0) var<uniform> params: ToneMapParams;

// Blanc diffus de référence en HDR (ITU-R BT.2408) : le niveau qui doit
// ressortir à « blanc SDR » après tone mapping. Sans cette normalisation, un
// signal PQ mis à l'échelle sur son pic (10000 nits) sature tout l'écran.
const SDR_WHITE_NITS: f32 = 203.0;

// ── EOTF inverses ───────────────────────────────────────────────────────────

// PQ (SMPTE ST.2084) : signal 0..1 → luminance absolue 0..10000 nits.
fn pq_eotf(n: vec3<f32>) -> vec3<f32> {
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let np  = pow(max(n, vec3<f32>(0.0)), vec3<f32>(1.0 / m2));
    let num = max(np - vec3<f32>(c1), vec3<f32>(0.0));
    let den = vec3<f32>(c2) - c3 * np;
    return pow(num / den, vec3<f32>(1.0 / m1)) * 10000.0;
}

// HLG (ARIB STD-B67) : signal 0..1 → scène linéaire 0..1, puis OOTF système
// (gamma 1.2) vers la luminance d'affichage rapportée au pic du contenu.
fn hlg_eotf(e: vec3<f32>, peak: f32) -> vec3<f32> {
    let a = 0.17883277;
    let b = 0.28466892;
    let c = 0.55991073;
    let lo = e * e / 3.0;
    let hi = (exp((e - vec3<f32>(c)) / a) + vec3<f32>(b)) / 12.0;
    let scene = select(hi, lo, e <= vec3<f32>(0.5));
    // Luminance de la scène (pondération BT.2020) pour l'OOTF.
    let yl = max(dot(scene, vec3<f32>(0.2627, 0.6780, 0.0593)), 1e-6);
    return scene * pow(yl, 0.2) * peak;
}

// ── Opérateurs de tone mapping (entrée normalisée : 1.0 = blanc diffus) ─────

fn aces_filmic(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn hable_partial(x: vec3<f32>) -> vec3<f32> {
    let A = 0.15; let B = 0.50; let C = 0.10;
    let D = 0.20; let E = 0.02; let F = 0.30;
    return ((x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F)) - vec3<f32>(E / F);
}
fn hable(v: vec3<f32>) -> vec3<f32> {
    return hable_partial(v * 2.0) / hable_partial(vec3<f32>(11.2));
}

// Reinhard étendu : préserve le blanc diffus à 1.0 et compresse seulement ce
// qui le dépasse, jusqu'au pic `peak` (rapporté au blanc diffus).
fn reinhard_extended(x: vec3<f32>, peak: f32) -> vec3<f32> {
    let p = max(peak, 1.0001);
    return x * (1.0 + x / (p * p)) / (1.0 + x);
}

// ── Gamut BT.2020 → BT.709 (lumière linéaire) ───────────────────────────────
fn bt2020_to_bt709(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        dot(c, vec3<f32>( 1.66049, -0.58764, -0.07285)),
        dot(c, vec3<f32>(-0.12455,  1.13290, -0.00835)),
        dot(c, vec3<f32>(-0.01815, -0.10058,  1.11873)),
    );
}

// ── Encodage sRGB (courbe exacte, pas l'approximation 2.2) ──────────────────
fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
    let x = f32((vi & 1u) * 2u) - 1.0;
    let y = 1.0 - f32((vi >> 1u) * 2u);
    return VOut(vec4<f32>(x, y, 0.0, 1.0), vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5));
}

@fragment fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let signal = clamp(textureSample(hdr_tex, samp, in.uv).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let peak_nits = max(params.max_luminance, SDR_WHITE_NITS);

    // 1. Signal encodé → luminance absolue (nits).
    var nits: vec3<f32>;
    if (params.transfer == 2u) {
        nits = hlg_eotf(signal, peak_nits);
    } else {
        nits = pq_eotf(signal);
    }

    // 2. Normalisation sur le blanc diffus : 1.0 = blanc SDR, pas le pic.
    var c = nits * (params.exposure / SDR_WHITE_NITS);
    let peak = peak_nits / SDR_WHITE_NITS;

    // 3. Tone mapping.
    var ldr: vec3<f32>;
    switch params.mode {
        case 1u:  { ldr = aces_filmic(c); }
        case 2u:  { ldr = hable(c); }
        default:  { ldr = reinhard_extended(c, peak); }
    }

    // 4. Gamut BT.2020 → BT.709, puis encodage sRGB.
    ldr = clamp(bt2020_to_bt709(ldr), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(srgb_encode(ldr), 1.0);
}
