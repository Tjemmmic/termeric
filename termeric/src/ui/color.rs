//! Operations on the `[r, g, b, a]` colors the renderer and UI layout draw
//! with. Channels are in `0.0..=1.0`, still sRGB-encoded.

/// `color` with its alpha replaced.
pub fn with_alpha(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] = alpha;
    color
}

/// `color` with its alpha multiplied by `factor`, for fading a color that
/// may already be translucent.
pub fn scale_alpha(mut color: [f32; 4], factor: f32) -> [f32; 4] {
    color[3] *= factor;
    color
}

/// `color` with its red, green, and blue multiplied by `factor` and clamped;
/// alpha is kept.
pub fn scale_rgb(color: [f32; 4], factor: f32) -> [f32; 4] {
    [
        (color[0] * factor).clamp(0.0, 1.0),
        (color[1] * factor).clamp(0.0, 1.0),
        (color[2] * factor).clamp(0.0, 1.0),
        color[3],
    ]
}

/// The color `t` of the way from `a` to `b`, with `t` clamped to `0..=1`.
pub fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|channel| a[channel] + (b[channel] - a[channel]) * t)
}

/// Perceived lightness on the sRGB-encoded channels: cheap, and enough to
/// tell a light theme from a dark one. WCAG contrast needs linearized
/// channels instead.
pub fn luminance(color: [f32; 4]) -> f32 {
    0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_is_set_or_scaled_without_touching_rgb() {
        let color = [0.2, 0.4, 0.6, 0.5];
        assert_eq!(with_alpha(color, 0.9), [0.2, 0.4, 0.6, 0.9]);
        assert_eq!(scale_alpha(color, 0.5), [0.2, 0.4, 0.6, 0.25]);
    }

    #[test]
    fn rgb_scaling_clamps_and_keeps_alpha() {
        assert_eq!(scale_rgb([0.5, 0.8, 0.0, 0.3], 1.5), [0.75, 1.0, 0.0, 0.3]);
    }

    #[test]
    fn mix_clamps_its_fraction() {
        let black = [0.0, 0.0, 0.0, 1.0];
        let white = [1.0, 1.0, 1.0, 1.0];
        assert_eq!(mix(black, white, 0.25), [0.25, 0.25, 0.25, 1.0]);
        assert_eq!(mix(black, white, -1.0), black);
        assert_eq!(mix(black, white, 2.0), white);
    }

    #[test]
    fn luminance_weights_green_most() {
        assert!(luminance([0.0, 1.0, 0.0, 1.0]) > luminance([1.0, 0.0, 0.0, 1.0]));
        assert!((luminance([1.0, 1.0, 1.0, 1.0]) - 1.0).abs() < 1e-6);
    }
}
