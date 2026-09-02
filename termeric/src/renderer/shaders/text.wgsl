// Text rendering shader

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coords: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) atlas_page: f32,
    @location(4) color_glyph: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) atlas_page: u32,
    @location(3) @interpolate(flat) color_glyph: u32,
}

struct Uniforms {
    screen_size: vec2<f32>,
    _padding: vec2<f32>,
}

@group(0) @binding(0)
var glyph_texture: texture_2d_array<f32>;

@group(0) @binding(1)
var glyph_sampler: sampler;

@group(0) @binding(2)
var<uniform> uniforms: Uniforms;

@group(0) @binding(3)
var color_glyph_texture: texture_2d_array<f32>;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    // Convert pixel coordinates to clip space (-1 to 1)
    let x = (in.position.x / uniforms.screen_size.x) * 2.0 - 1.0;
    let y = 1.0 - (in.position.y / uniforms.screen_size.y) * 2.0;

    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    out.tex_coords = in.tex_coords;
    out.color = in.color;
    out.atlas_page = u32(in.atlas_page);
    out.color_glyph = u32(in.color_glyph);

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if in.color_glyph != 0u {
        let rgba = textureSample(
            color_glyph_texture,
            glyph_sampler,
            in.tex_coords,
            in.atlas_page,
        );
        return vec4<f32>(rgba.rgb, rgba.a * in.color.a);
    }
    let alpha = textureSample(glyph_texture, glyph_sampler, in.tex_coords, in.atlas_page).r;
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
