//! The game's colour grading for the 3D preview.
//!
//! Smash draws its scene (models and effects) at half intensity into a linear buffer, adds
//! bloom, and in its last pass maps each pixel through the stage's 16x16x16 colour grading LUT
//! in gamma space and scales by 1.3703 there (x2 linear). The model renderer (ssbh_wgpu) ports
//! that pass; this module supplies the LUT the training stage uses and the inverse of the pass,
//! for clearing the scene to a backdrop that comes out a chosen colour.

/// The training stage's grading LUT, read from the game's GPU memory in a capture of a
/// training frame (13.0.4, Ryujinx + RenderDoc). It is separable and the same for all three
/// channels, so this one curve is the whole table: output at input i/15.
const TRAINING_CURVE: [u8; 16] = [
    0, 15, 30, 46, 64, 82, 101, 121, 140, 158, 176, 193, 209, 224, 240, 255,
];

/// The LUT as RGBA8 texels, 16x16x16 with red varying fastest.
pub fn training_lut() -> Vec<u8> {
    let mut texels = Vec::with_capacity(16 * 16 * 16 * 4);
    for b in TRAINING_CURVE {
        for g in TRAINING_CURVE {
            for r in TRAINING_CURVE {
                texels.extend([r, g, b, 255]);
            }
        }
    }
    texels
}

/// The curve sampled as the LUT is, with linear filtering between texel centres.
fn curve(gamma: f32) -> f32 {
    let at = (gamma.clamp(0.0, 1.0) * 15.0).min(15.0);
    let low = at.floor() as usize;
    let high = (low + 1).min(15);
    let t = at - low as f32;
    (TRAINING_CURVE[low] as f32 * (1.0 - t) + TRAINING_CURVE[high] as f32 * t) / 255.0
}

/// One channel through the post pass with no bloom: a scene value to the linear output.
/// Mirrors ssbh_wgpu's `GetPostProcessingResult`.
pub fn post(scene: f32) -> f32 {
    let gamma = scene.max(0.0).powf(0.454_545);
    let graded = (curve(gamma) - gamma) * 0.99961 + gamma;
    (graded * 1.3703).powf(2.2)
}

/// The scene value each channel must hold to come out of the post pass as `linear`.
pub fn scene_value_for(linear: [f32; 3]) -> [f32; 3] {
    linear.map(|want| {
        let (mut low, mut high) = (0.0f32, 1.0f32);
        for _ in 0..40 {
            let mid = (low + high) * 0.5;
            if post(mid) < want {
                low = mid;
            } else {
                high = mid;
            }
        }
        (low + high) * 0.5
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_reproduces_the_captured_table_at_its_texel_centres() {
        let lut = training_lut();
        // Spot cells from the capture, including off-diagonal ones the separable form claims.
        let cell = |r: usize, g: usize, b: usize| {
            let i = ((b * 16 + g) * 16 + r) * 4;
            [lut[i], lut[i + 1], lut[i + 2]]
        };
        assert_eq!(cell(10, 0, 0), [176, 0, 0]);
        assert_eq!(cell(15, 15, 15), [255, 255, 255]);
        assert_eq!(cell(3, 7, 12), [46, 121, 209]);
        for (i, value) in TRAINING_CURVE.iter().enumerate() {
            assert!((curve(i as f32 / 15.0) * 255.0 - *value as f32).abs() < 1e-3);
        }
    }

    #[test]
    fn half_scale_white_comes_out_white() {
        // The scene holds half intensity; the post pass's x1.3703 in gamma space is x2 linear.
        assert!((1.3703f32.powf(2.2) - 2.0).abs() < 0.001);
        // With the training curve's contrast, half-scale white lands just past 1 (white).
        assert!(post(0.5) >= 1.0 && post(0.5) < 1.1, "{}", post(0.5));
        assert_eq!(post(0.0), 0.0);
    }

    #[test]
    fn the_backdrop_solve_inverts_the_post_pass() {
        let want = [0.05, 0.05, 0.1];
        let scene = scene_value_for(want);
        for (s, w) in scene.iter().zip(want) {
            assert!((post(*s) - w).abs() < 1e-4, "{s} -> {} not {w}", post(*s));
        }
        // And the grading is not the identity it would be without the LUT: the solved value
        // differs from plain halving.
        assert!((scene[2] - 0.05).abs() > 1e-3, "{scene:?}");
    }
}
