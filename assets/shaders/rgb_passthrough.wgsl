// Affichage direct d'une texture RGB déjà convertie par le GPU.
//
// Sert au chemin sans copie : le processeur vidéo D3D11 a déjà fait la matrice
// YUV→RGB dans une texture partagée, il ne reste qu'à l'échantillonner. Même
// moyenne 3×3 qu'ailleurs pour ne pas faire fourmiller les réductions.

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var samp:    sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
    let x = f32((vi & 1u) * 2u) - 1.0;
    let y = 1.0 - f32((vi >> 1u) * 2u);
    return VOut(vec4<f32>(x, y, 0.0, 1.0), vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5));
}

fn sample_box(uv: vec2<f32>, fw: vec2<f32>) -> vec3<f32> {
    let o = fw / 3.0;
    var acc = vec3<f32>(0.0);
    for (var j: i32 = -1; j <= 1; j = j + 1) {
        for (var i: i32 = -1; i <= 1; i = i + 1) {
            acc = acc + textureSampleLevel(src_tex, samp, uv + vec2<f32>(f32(i), f32(j)) * o, 0.0).rgb;
        }
    }
    return acc / 9.0;
}

@fragment fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let fw = fwidth(in.uv);
    return vec4<f32>(sample_box(in.uv, fw), 1.0);
}
