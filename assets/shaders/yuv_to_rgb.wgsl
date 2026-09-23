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

// ─── Fragment shader ────────────────────────────────────────────────────────

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let y_raw = textureSample(y_tex, samp, in.tex_coord).r;

    // Deux dispositions de chroma :
    //  - planaire (YUV420P / YUV420P10LE) : U et V dans deux textures R.
    //  - semi-planaire (NV12 / P010, sortie directe du décodeur matériel) :
    //    U et V entrelacés dans une seule texture RG, liée aux deux slots.
    // `color.offset.x` porte le drapeau (uniforme, donc branche sûre pour
    // textureSample).
    let uv_tex = textureSample(u_tex, samp, in.tex_coord);
    var u_raw = uv_tex.r;
    var v_raw = uv_tex.g;
    if (color.offset.x < 0.5) {
        v_raw = textureSample(v_tex, samp, in.tex_coord).r;
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
