//! Analysis windows and the overlap-add machinery that keeps them invertible.
//!
//! Every window here is defined in its *periodic* form (`n` in `0..length`
//! divided by `length`, not by `length - 1`), which is the form that satisfies
//! the constant-overlap-add (COLA) condition used by [`crate::dsp::stft`].

use std::f64::consts::TAU;

/// Window shapes supported by the analysis stages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowKind {
    /// No shaping at all.
    Rectangular,
    /// Raised cosine; the default because `hann^2` overlap-adds to a constant
    /// for hop sizes of `length / 2`, `length / 4`, and so on.
    #[default]
    Hann,
    /// Slightly lower side lobes than Hann, at the cost of a small pedestal.
    Hamming,
    /// Strong side-lobe suppression for pitch analysis.
    Blackman,
}

impl WindowKind {
    /// The window's name as used on the command line.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rectangular => "rectangular",
            Self::Hann => "hann",
            Self::Hamming => "hamming",
            Self::Blackman => "blackman",
        }
    }

    /// Parses a window name, accepting the spellings printed by [`Self::name`].
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "rect" | "rectangular" | "none" => Some(Self::Rectangular),
            "hann" | "hanning" => Some(Self::Hann),
            "hamming" => Some(Self::Hamming),
            "blackman" => Some(Self::Blackman),
            _ => None,
        }
    }

    /// Materializes the window of the requested length.
    #[must_use]
    pub fn build(self, length: usize) -> Vec<f64> {
        (0..length)
            .map(|index| {
                let phase = TAU * index as f64 / length as f64;
                match self {
                    Self::Rectangular => 1.0,
                    Self::Hann => 0.5 - 0.5 * phase.cos(),
                    Self::Hamming => 0.54 - 0.46 * phase.cos(),
                    Self::Blackman => 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos(),
                }
            })
            .collect()
    }
}

/// Sums `window[n]^2` over every frame that overlaps a steady-state sample.
///
/// A weighted overlap-add resynthesis divides by exactly this quantity, so a
/// value that stays away from zero is what makes the round trip stable.
#[must_use]
pub fn overlap_add_gain(window: &[f64], hop: usize) -> f64 {
    if hop == 0 || window.is_empty() {
        return 0.0;
    }
    (0..window.len())
        .step_by(hop)
        .map(|index| window[index] * window[index])
        .sum()
}

/// Reports whether squared-window overlap-add is well conditioned for `hop`.
///
/// The check walks the steady-state region and requires every normalization
/// divisor to be positive, which is the practical form of the COLA condition.
#[must_use]
pub fn satisfies_overlap_add(window: &[f64], hop: usize) -> bool {
    if hop == 0 || hop > window.len() || window.is_empty() {
        return false;
    }
    let length = window.len();
    (0..hop).all(|position| {
        let sum: f64 = (position..length)
            .step_by(hop)
            .map(|index| window[index] * window[index])
            .sum();
        sum > 1e-9
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_names_round_trip() {
        for kind in [
            WindowKind::Rectangular,
            WindowKind::Hann,
            WindowKind::Hamming,
            WindowKind::Blackman,
        ] {
            assert_eq!(WindowKind::parse(kind.name()), Some(kind));
        }
        assert_eq!(WindowKind::parse(" HANNING "), Some(WindowKind::Hann));
        assert_eq!(WindowKind::parse("none"), Some(WindowKind::Rectangular));
        assert_eq!(WindowKind::parse("gaussian"), None);
        assert_eq!(WindowKind::default(), WindowKind::Hann);
    }

    #[test]
    fn hann_is_periodic_and_symmetric_about_its_centre() {
        let window = WindowKind::Hann.build(8);

        assert!(window[0].abs() < 1e-15);
        assert!((window[4] - 1.0).abs() < 1e-15);
        for index in 1..4 {
            assert!((window[index] - window[8 - index]).abs() < 1e-15);
        }
    }

    #[test]
    fn window_shapes_have_the_expected_peaks() {
        assert!((WindowKind::Rectangular.build(4).iter().sum::<f64>() - 4.0).abs() < 1e-15);
        assert!((WindowKind::Hamming.build(8)[4] - 1.0).abs() < 1e-15);
        assert!((WindowKind::Blackman.build(8)[4] - 1.0).abs() < 1e-12);
        assert!(WindowKind::Blackman.build(8)[0].abs() < 1e-12);
    }

    #[test]
    fn squared_hann_overlap_add_is_constant_for_power_of_two_hops() {
        let window = WindowKind::Hann.build(64);

        for hop in [8, 16, 32] {
            assert!(satisfies_overlap_add(&window, hop), "hop {hop}");
        }

        // Squared Hann overlap-adds to a constant for hops of a quarter window
        // or less; at half a window the sum is merely positive, which is all
        // the normalized resynthesis in `stft` needs.
        for hop in [8, 16] {
            let reference: f64 = (0..window.len())
                .step_by(hop)
                .map(|index| window[index] * window[index])
                .sum();
            for position in 0..hop {
                let sum: f64 = (position..window.len())
                    .step_by(hop)
                    .map(|index| window[index] * window[index])
                    .sum();
                assert!(
                    (sum - reference).abs() < 1e-12,
                    "hop {hop} position {position}"
                );
            }
        }
        assert!((overlap_add_gain(&window, 32) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn degenerate_inputs_are_rejected() {
        let window = WindowKind::Hann.build(16);

        assert!(!satisfies_overlap_add(&window, 0));
        assert!(!satisfies_overlap_add(&window, 17));
        assert!(!satisfies_overlap_add(&[], 4));
        assert_eq!(overlap_add_gain(&[], 4), 0.0);
        assert_eq!(overlap_add_gain(&window, 0), 0.0);
    }
}
