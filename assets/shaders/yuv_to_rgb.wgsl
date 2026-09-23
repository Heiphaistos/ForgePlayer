// ─── Vertex shader ──────────────────────────────────────────────────────────

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let x = f32((vi & 1u) * 2u) - 1.0;
    let y = 1.0 - f32((vi >> 1u) * 2u);
    var out: VertexOutput;
    out.position  = vec4<f32>(x, y, 0.0, 1.0);
    out.tex_coord = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

// ─── Bindings ───────────────────────────────────────────────────────────────

@group(0) @binding(0) var y_tex: texture_2d<f32>;
@group(0) @binding(1) var u_tex: texture_2d<f32>;
@group(0) @binding(2) var v_tex: texture_2d<f32>;
@group(0) @binding(3) var samp:  sampler;

// Color transform uniform — column-major layout matching WGSL mat4x4.
// The Rust side passes columns as [[f32;4];4] arrays.
struct ColorTransform {
    matrix: mat4x4<f32>,
    offset: vec4<f32>,
};
@group(1) @binding(0) var<uniform> color: ColorTransform;

// Moyenne 3×3 sur l'empreinte réelle du pixel à l'écran.
//
// L'échantillonnage bilinéaire ne moyenne que 2×2 texels : quand une image 4K
// est affichée dans une fenêtre trois fois plus petite, les deux tiers des
// texels ne sont jamais lus et l'image fourmille (mesuré : +32 % d'énergie
// hautes fréquences par rapport à un Lanczos correct). Les décalages viennent
// de `fwidth`, calculé une seule fois en flux uniforme puis passé ici —
// `textureSampleLevel` n'a pas besoin de dérivées, donc l'appel reste légal.
fn sample_box(t: texture_2d<f32>, uv: vec2<f32>, fw: vec2<f32>) -> vec4<f32> {
    let o = fw / 3.0;
    var acc = vec4<f32>(0.0);
    for (var j: i32 = -1; j <= 1; j = j + 1) {
        for (var i: i32 = -1; i <= 1; i = i + 1) {
            acc = acc + textureSampleLevel(t, samp, uv + vec2<f32>(f32(i), f32(j)) * o, 0.0);
        }
    }
    return acc / 9.0;
}

// ─── Fragment shader ────────────────────────────────────────────────────────

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // `color.offset.z` remet les échantillons à l'échelle : le 10-bit planaire
    // de FFmpeg range sa valeur dans les bits BAS du mot 16 bits, la texture
    // R16Unorm la normalise donc sur 65535 au lieu de 1023.
    let scale = color.offset.z;
    // Empreinte du pixel en coordonnées de texture : calculée ici, en flux de
    // contrôle uniforme, avant toute branche.
    let fw = fwidth(in.tex_coord);
    let y_raw = sample_box(y_tex, in.tex_coord, fw).r * scale;

    // Deux dispositions de chroma :
    //  - planaire (YUV420P / YUV420P10LE) : U et V dans deux textures R.
    //  - semi-planaire (NV12 / P010, sortie directe du décodeur matériel) :
    //    U et V entrelacés dans une seule texture RG, liée aux deux slots.
    // `color.offset.x` porte le drapeau (uniforme, donc branche sûre pour
    // textureSample).
    let uv_tex = sample_box(u_tex, in.tex_coord, fw) * scale;
    var u_raw = uv_tex.r;
    var v_raw = uv_tex.g;
    if (color.offset.x < 0.5) {
        v_raw = sample_box(v_tex, in.tex_coord, fw).r * scale;
    }

    // `color.offset.y` porte l'offset de luma : 16/255 en plage limitée
    // (MPEG/TV), 0 en plage complète (JPEG/PC). La chroma est centrée sur
    // 128/255 dans les deux cas. Les facteurs d'échelle sont dans la matrice.
    let y = y_raw - color.offset.y;
    let u = u_raw - 128.0 / 255.0;
    let v = v_raw - 128.0 / 255.0;

    // Matrix multiplication (column-major): result = M * [y, u, v, 1]
    let rgb = color.matrix * vec4<f32>(y, u, v, 1.0);

    return vec4<f32>(clamp(rgb.x, 0.0, 1.0), clamp(rgb.y, 0.0, 1.0), clamp(rgb.z, 0.0, 1.0), 1.0);
}
