// ─── HDR Tone Mapping (PQ / HLG → SDR) ──────────────────────────────────────
// Post-process appliqué à la texture RGB encodée (PQ ou HLG) produite par la
// première passe YUV→RGB. Chaîne identique à celle de VLC/libplacebo :
//   signal encodé → lumière linéaire (nits) → normalisation blanc diffus
//   → tone mapping → conversion de gamut BT.2020 → BT.709 → encodage sRGB.

@group(0) @binding(0) var hdr_tex: texture_2d<f32>;
@group(0) @binding(1) var samp:    sampler;

struct ToneMapParams {
    mode:          u32,   // 0=Reinhard étendu (défaut, calé sur VLC), 1=ACES, 2=Hable, 3=neutre
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

fn aces_filmic(x: f32) -> f32 {
    let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}

fn hable_partial(x: f32) -> f32 {
    let A = 0.15; let B = 0.50; let C = 0.10;
    let D = 0.20; let E = 0.02; let F = 0.30;
    return ((x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F)) - (E / F);
}
fn hable(v: f32) -> f32 {
    return hable_partial(v * 2.0) / hable_partial(11.2);
}

// Courbe neutre : identité sous le genou, puis compression asymptotique vers
// 1.0. C'est le comportement attendu d'un tone mapping HDR→SDR (celui de
// libplacebo/VLC) : le blanc diffus et tout ce qui est en dessous sort
// EXACTEMENT comme en SDR, seuls les hautes lumières au-dessus sont ramenées.
// Les courbes filmiques (ACES, Hable) sont scene-referred : elles remontent
// aussi les tons moyens, ce qui délave l'image (mesuré : +0,17 sur un aplat
// bleu par rapport à VLC).
fn neutral_knee(l: f32) -> f32 {
    let k = 0.75;
    if (l <= k) { return l; }
    let s = 1.0 - k;
    return k + s * (1.0 - exp(-(l - k) / s));
}

// Reinhard étendu : préserve le blanc diffus à 1.0 et compresse seulement ce
// qui le dépasse, jusqu'au pic `peak` (rapporté au blanc diffus).
fn reinhard_extended(x: f32, peak: f32) -> f32 {
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

// Moyenne 3×3 sur l'empreinte réelle du pixel. C'est ICI que le contenu HDR
// est réduit : la première passe rend à la résolution de la source dans une
// texture hors écran, cette passe-ci l'amène à la taille de la fenêtre. Sans
// cette moyenne, seuls 2×2 texels sur les 9 d'une réduction 3× sont lus et
// l'image fourmille.
fn sample_box(uv: vec2<f32>, fw: vec2<f32>) -> vec3<f32> {
    let o = fw / 3.0;
    var acc = vec3<f32>(0.0);
    for (var j: i32 = -1; j <= 1; j = j + 1) {
        for (var i: i32 = -1; i <= 1; i = i + 1) {
            acc = acc + textureSampleLevel(hdr_tex, samp, uv + vec2<f32>(f32(i), f32(j)) * o, 0.0).rgb;
        }
    }
    return acc / 9.0;
}

@fragment fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let fw = fwidth(in.uv);
    let signal = clamp(sample_box(in.uv, fw), vec3<f32>(0.0), vec3<f32>(1.0));
    let peak_nits = max(params.max_luminance, SDR_WHITE_NITS);

    // 1. Signal encodé → luminance absolue (nits).
    var nits: vec3<f32>;
    if (params.transfer == 2u) {
        nits = hlg_eotf(signal, peak_nits);
    } else {
        nits = pq_eotf(signal);
    }

    // 2. Normalisation sur le blanc diffus : 1.0 = blanc SDR, pas le pic.
    let c = nits * (params.exposure / SDR_WHITE_NITS);
    let peak = peak_nits / SDR_WHITE_NITS;

    // 3. Tone mapping sur la LUMINANCE seule, la chrominance suit le même
    //    rapport. Compresser chaque canal séparément délave les aplats
    //    colorés (le canal le plus fort sature avant les autres).
    let l = max(dot(c, vec3<f32>(0.2627, 0.6780, 0.0593)), 1e-6);
    var lm: f32;
    switch params.mode {
        case 0u:  { lm = reinhard_extended(l, peak); }
        case 1u:  { lm = aces_filmic(l); }
        case 2u:  { lm = hable(l); }
        default:  { lm = neutral_knee(l); }
    }
    var ldr = c * (lm / l);

    // 4. Gamut BT.2020 → BT.709, puis encodage sRGB.
    ldr = clamp(bt2020_to_bt709(ldr), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(srgb_encode(ldr), 1.0);
}
