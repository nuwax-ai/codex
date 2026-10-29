//! The Nuwax mark's studio lighting: the icon's gradient (teal-green lower
//! left, blue across the top, rose on the right) read off the mark's own
//! surface, shaded with a fixed key light. Adapted to the terminal's
//! foreground and background so the saturated mark keeps its contrast on a
//! light theme.

use crate::color::is_light;

/// The icon's three gradient stops, in 0-255 channel order.
const GREEN: [f64; 3] = [56.0, 208.0, 148.0];
const BLUE: [f64; 3] = [66.0, 133.0, 244.0];
const ROSE: [f64; 3] = [244.0, 63.0, 94.0];
/// The mark's core: near-white on a dark theme, deep ink on a light one.
const WHITE: [f64; 3] = [246.0, 249.0, 252.0];
const INK: [f64; 3] = [26.0, 32.0, 44.0];

pub(super) struct Lighting {
    pub(super) background: [f64; 3],
    /// Hue scale: full on a dark theme, pulled down on a light one.
    hue_gain: f64,
    /// The core highlight, chosen for contrast against `background`.
    highlight: [f64; 3],
}

impl Lighting {
    pub(super) fn terminal(_fg: (u8, u8, u8), bg: (u8, u8, u8)) -> Self {
        let background = [bg.0.into(), bg.1.into(), bg.2.into()];
        if is_light(bg) {
            Self {
                background,
                hue_gain: 0.68,
                highlight: INK,
            }
        } else {
            Self {
                background,
                hue_gain: 1.0,
                highlight: WHITE,
            }
        }
    }

    /// The mark's core color for the current theme, the target the spark's
    /// hue fades to during the morph.
    pub(super) fn core(&self) -> [f64; 3] {
        self.highlight
    }

    pub(super) fn shade(
        &self,
        [nx, ny, nz]: [f64; 3],
        depth: f64,
        grain: f64,
        hue: [f64; 3],
    ) -> u32 {
        let diffuse = (nx * -0.410 + ny * -0.564 + nz * 0.718 + grain)
            .clamp(/*min*/ 0.0, /*max*/ 1.0);
        let bounce = (nx * 0.55 + ny * 0.2 - nz * 0.35).clamp(/*min*/ 0.0, /*max*/ 1.0)
            * (1.0 - diffuse)
            * 0.38;
        let edge = (1.0 - nz.clamp(/*min*/ -1.0, /*max*/ 1.0).abs()).powf(/*n*/ 2.4)
            * (nx * 0.85 - ny * 0.38).clamp(/*min*/ 0.0, /*max*/ 1.0)
            * 0.82;
        let gloss = (nx * -0.220 + ny * -0.302 + nz * 0.928)
            .clamp(/*min*/ 0.0, /*max*/ 1.0)
            .powi(/*n*/ 18)
            * 0.96;
        let base = (1.0 - bounce) * (1.0 - edge) * (1.0 - gloss);
        let gain = (0.90 + depth * 0.18).clamp(/*min*/ 0.68, /*max*/ 1.0);
        // The palette is derived from the local hue so the gradient survives
        // the lighting terms: a deepened shadow, a blue-leaning fill, a
        // rose-leaning rim, and the mark's own core as the specular highlight.
        let scaled = |channel: usize| hue[channel] * self.hue_gain;
        let mix = |from: [f64; 3], to: [f64; 3], t: f64| {
            [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
                from[2] + (to[2] - from[2]) * t,
            ]
        };
        let key = [scaled(0), scaled(1), scaled(2)];
        let shadow = [
            key[0] * 0.30,
            key[1] * 0.32,
            key[2] * 0.42,
        ];
        let fill = mix(key, [BLUE[0] * self.hue_gain, BLUE[1] * self.hue_gain, BLUE[2] * self.hue_gain], 0.30);
        let rim = mix(key, [ROSE[0] * self.hue_gain, ROSE[1] * self.hue_gain, ROSE[2] * self.hue_gain], 0.45);
        let highlight = self.highlight;
        let rgb: [u8; 3] = std::array::from_fn(|i| {
            let value = shadow[i] * (1.0 - diffuse) * base
                + key[i] * diffuse * base
                + fill[i] * bounce * (1.0 - edge) * (1.0 - gloss)
                + rim[i] * edge * (1.0 - gloss)
                + highlight[i] * gloss;
            (self.background[i] + (value - self.background[i]) * gain).round() as u8
        });
        u32::from_be_bytes([0, rgb[0], rgb[1], rgb[2]])
    }
}

/// The mark's gradient at one surface point. `u` runs left (-1) to right (1)
/// and `v` top (-1) to bottom (1) in the renderer's normalized space, the
/// same layout the app icon uses: rose on the right, blue across the top,
/// teal-green in the lower left. A weighted brand floor keeps every region
/// inside the family instead of falling back to black where no stop dominates.
pub(super) fn brand_hue(u: f64, v: f64) -> [f64; 3] {
    let ramp = |value: f64| {
        let t = value.clamp(/*min*/ 0.0, /*max*/ 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let weights = [ramp((-u + v) * 0.5), ramp(-v), ramp(u)];
    const FLOOR: f64 = 0.22;
    let total = weights[0] + weights[1] + weights[2] + FLOOR;
    let mut hue = [0.0; 3];
    for (index, color) in [GREEN, BLUE, ROSE].iter().enumerate() {
        for channel in 0..3 {
            hue[channel] += color[channel] * weights[index];
        }
    }
    for channel in 0..3 {
        hue[channel] = (hue[channel] + BLUE[channel] * FLOOR) / total;
    }
    hue
}

/// Fades a hue toward the mark's core, used where the icon is solid white
/// rather than gradient (the spark) so the morph changes color with shape.
pub(super) fn toward_core(hue: [f64; 3], core: [f64; 3], amount: f64) -> [f64; 3] {
    [
        hue[0] + (core[0] - hue[0]) * amount,
        hue[1] + (core[1] - hue[1]) * amount,
        hue[2] + (core[2] - hue[2]) * amount,
    ]
}
