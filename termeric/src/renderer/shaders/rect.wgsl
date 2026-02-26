// Rectangle and line rendering shader with SDF-based anti-aliasing

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) rect_center: vec2<f32>,
    @location(3) rect_half_size: vec2<f32>,
    @location(4) corner_radius: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) pixel_pos: vec2<f32>,
    @location(2) rect_center: vec2<f32>,
    @location(3) rect_half_size: vec2<f32>,
    @location(4) corner_radius: f32,
}

struct Uniforms {
    screen_size: vec2<f32>,
    _padding: vec2<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    // Convert pixel coordinates to clip space (-1 to 1)
    let x = (in.position.x / uniforms.screen_size.x) * 2.0 - 1.0;
    let y = 1.0 - (in.position.y / uniforms.screen_size.y) * 2.0;

    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    out.color = in.color;
    out.pixel_pos = in.position;
    out.rect_center = in.rect_center;
    out.rect_half_size = in.rect_half_size;
    out.corner_radius = in.corner_radius;

    return out;
}

// SDF for a rounded rectangle
fn sdf_rounded_rect(p: vec2<f32>, center: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let d = abs(p - center) - half_size + vec2<f32>(radius);
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - radius;
}

// SDF for a line segment from point a to point b
fn sdf_line_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Negative corner_radius signals a line segment:
    //   rect_center = line start point (a)
    //   rect_half_size = line end point (b)
    //   abs(corner_radius) = half-thickness of the line
    if in.corner_radius < 0.0 {
        let half_thickness = -in.corner_radius;
        let dist = sdf_line_segment(in.pixel_pos, in.rect_center, in.rect_half_size) - half_thickness;
        let alpha = 1.0 - smoothstep(-0.5, 0.5, dist);
        if alpha <= 0.0 {
            discard;
        }
        return vec4<f32>(in.color.rgb, in.color.a * alpha);
    }

    // If corner_radius is 0, skip SDF (fast path for sharp rects)
    if in.corner_radius <= 0.0 {
        return in.color;
    }

    let dist = sdf_rounded_rect(in.pixel_pos, in.rect_center, in.rect_half_size, in.corner_radius);

    // Anti-aliased edge: smoothstep over ~1px
    let alpha = 1.0 - smoothstep(-0.5, 0.5, dist);

    if alpha <= 0.0 {
        discard;
    }

    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
