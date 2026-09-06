//! Complex arithmetic and an in-place radix-2 fast Fourier transform.
//!
//! The transform is written out rather than pulled from a crate so the whole
//! analysis path stays dependency-free and deterministic: the same input
//! produces the same bits on every target this crate is tested on.

use std::ops::{Add, Mul, Sub};

/// A complex number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl Complex {
    /// A complex number from its parts.
    #[must_use]
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    /// A real number as a complex number.
    #[must_use]
    pub const fn real(re: f64) -> Self {
        Self { re, im: 0.0 }
    }

    /// `e^(i * angle)`.
    #[must_use]
    pub fn from_angle(angle: f64) -> Self {
        Self {
            re: angle.cos(),
            im: angle.sin(),
        }
    }

    /// Magnitude.
    #[must_use]
    pub fn magnitude(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// Squared magnitude, avoiding the square root.
    #[must_use]
    pub fn power(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    /// Phase angle in radians.
    #[must_use]
    pub fn phase(self) -> f64 {
        self.im.atan2(self.re)
    }

    /// Complex conjugate.
    #[must_use]
    pub const fn conjugate(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }

    /// Multiplies by a real factor.
    #[must_use]
    pub const fn scale(self, factor: f64) -> Self {
        Self {
            re: self.re * factor,
            im: self.im * factor,
        }
    }
}

impl Add for Complex {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.re + other.re, self.im + other.im)
    }
}

impl Sub for Complex {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.re - other.re, self.im - other.im)
    }
}

impl Mul for Complex {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        Self::new(
            self.re * other.re - self.im * other.im,
            self.re * other.im + self.im * other.re,
        )
    }
}

/// Direction of a transform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Time domain to frequency domain.
    Forward,
    /// Frequency domain to time domain, including the `1/n` scaling.
    Inverse,
}

/// Runs an in-place radix-2 FFT over `buffer`, whose length must be a power of
/// two.
///
/// # Panics
///
/// Panics if the buffer length is not a power of two.
pub fn transform(buffer: &mut [Complex], direction: Direction) {
    let n = buffer.len();
    assert!(
        n.is_power_of_two(),
        "FFT length {n} is not a power of two; pad with `next_power_of_two`"
    );
    if n <= 1 {
        return;
    }

    bit_reverse_permute(buffer);

    let sign = if direction == Direction::Forward {
        -1.0
    } else {
        1.0
    };
    let mut length = 2;
    while length <= n {
        let angle = sign * std::f64::consts::TAU / length as f64;
        let step = Complex::from_angle(angle);
        for start in (0..n).step_by(length) {
            let mut factor = Complex::real(1.0);
            for offset in 0..length / 2 {
                let even = buffer[start + offset];
                let odd = buffer[start + offset + length / 2] * factor;
                buffer[start + offset] = even + odd;
                buffer[start + offset + length / 2] = even - odd;
                factor = factor * step;
            }
        }
        length *= 2;
    }

    if direction == Direction::Inverse {
        let scale = 1.0 / n as f64;
        for value in buffer.iter_mut() {
            *value = value.scale(scale);
        }
    }
}

fn bit_reverse_permute(buffer: &mut [Complex]) {
    let n = buffer.len();
    let mut target = 0;
    for source in 1..n {
        let mut bit = n >> 1;
        while target & bit != 0 {
            target ^= bit;
            bit >>= 1;
        }
        target |= bit;
        if source < target {
            buffer.swap(source, target);
        }
    }
}

/// Transforms a real signal, returning the full complex spectrum.
#[must_use]
pub fn forward_real(samples: &[f64]) -> Vec<Complex> {
    let mut buffer: Vec<Complex> = samples.iter().copied().map(Complex::real).collect();
    buffer.resize(samples.len().next_power_of_two().max(1), Complex::default());
    transform(&mut buffer, Direction::Forward);
    buffer
}

/// Magnitude spectrum of a real signal, keeping the `n / 2 + 1` unique bins.
#[must_use]
pub fn magnitude_spectrum(samples: &[f64]) -> Vec<f64> {
    let spectrum = forward_real(samples);
    let bins = spectrum.len() / 2 + 1;
    spectrum[..bins].iter().map(|bin| bin.magnitude()).collect()
}

/// Circular cross-correlation of two real signals, computed through the FFT.
///
/// Element `k` of the result is `sum_n a[n + k] * b[n]`, with both inputs
/// zero-padded to a common power-of-two length, so a peak at index `k` means
/// `b` lines up with `a` when shifted right by `k` samples.
#[must_use]
pub fn cross_correlate(a: &[f64], b: &[f64]) -> Vec<f64> {
    let length = (a.len() + b.len()).next_power_of_two().max(1);
    let mut left: Vec<Complex> = a.iter().copied().map(Complex::real).collect();
    let mut right: Vec<Complex> = b.iter().copied().map(Complex::real).collect();
    left.resize(length, Complex::default());
    right.resize(length, Complex::default());

    transform(&mut left, Direction::Forward);
    transform(&mut right, Direction::Forward);
    for (value, other) in left.iter_mut().zip(right.iter()) {
        *value = *value * other.conjugate();
    }
    transform(&mut left, Direction::Inverse);
    left.into_iter().map(|value| value.re).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_dft(samples: &[f64]) -> Vec<Complex> {
        let n = samples.len();
        (0..n)
            .map(|bin| {
                samples
                    .iter()
                    .enumerate()
                    .fold(Complex::default(), |sum, (index, sample)| {
                        let angle = -std::f64::consts::TAU * bin as f64 * index as f64 / n as f64;
                        sum + Complex::from_angle(angle).scale(*sample)
                    })
            })
            .collect()
    }

    #[test]
    fn complex_arithmetic_matches_the_definitions() {
        let a = Complex::new(1.0, 2.0);
        let b = Complex::new(3.0, -1.0);

        assert_eq!(a + b, Complex::new(4.0, 1.0));
        assert_eq!(a - b, Complex::new(-2.0, 3.0));
        assert_eq!(a * b, Complex::new(5.0, 5.0));
        assert_eq!(a.conjugate(), Complex::new(1.0, -2.0));
        assert_eq!(a.scale(2.0), Complex::new(2.0, 4.0));
        assert_eq!(a.power(), 5.0);
        assert!((a.magnitude() - 5.0_f64.sqrt()).abs() < 1e-15);
        assert!((Complex::new(0.0, 1.0).phase() - std::f64::consts::FRAC_PI_2).abs() < 1e-15);
        assert_eq!(Complex::real(3.0), Complex::new(3.0, 0.0));
    }

    #[test]
    fn forward_transform_matches_a_naive_dft() {
        let samples: Vec<f64> = (0..16)
            .map(|index| (f64::from(index) * 0.7).sin() + 0.25 * (f64::from(index) * 2.1).cos())
            .collect();
        let expected = naive_dft(&samples);
        let actual = forward_real(&samples);

        for (index, (left, right)) in expected.iter().zip(actual.iter()).enumerate() {
            assert!((left.re - right.re).abs() < 1e-9, "bin {index} real part");
            assert!((left.im - right.im).abs() < 1e-9, "bin {index} imaginary");
        }
    }

    #[test]
    fn inverse_transform_undoes_the_forward_transform() {
        let samples: Vec<f64> = (0..64)
            .map(|index| (f64::from(index) * 0.31).sin())
            .collect();
        let mut buffer = forward_real(&samples);
        transform(&mut buffer, Direction::Inverse);

        for (original, restored) in samples.iter().zip(buffer.iter()) {
            assert!((original - restored.re).abs() < 1e-12);
            assert!(restored.im.abs() < 1e-12);
        }
    }

    #[test]
    fn a_pure_tone_peaks_in_its_own_bin() {
        let n = 64;
        let bin = 5;
        let samples: Vec<f64> = (0..n)
            .map(|index| (std::f64::consts::TAU * bin as f64 * index as f64 / n as f64).cos())
            .collect();
        let spectrum = magnitude_spectrum(&samples);
        let peak = spectrum
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, _)| index)
            .unwrap();

        assert_eq!(peak, bin);
        assert_eq!(spectrum.len(), n / 2 + 1);
        assert!((spectrum[bin] - n as f64 / 2.0).abs() < 1e-9);
    }

    #[test]
    fn cross_correlation_finds_the_shift() {
        let pattern: Vec<f64> = (0..8).map(|index| f64::from(index) - 3.5).collect();
        let mut signal = vec![0.0; 5];
        signal.extend_from_slice(&pattern);
        signal.extend(std::iter::repeat_n(0.0, 11));

        let correlation = cross_correlate(&signal, &pattern);
        let peak = correlation
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, _)| index)
            .unwrap();

        assert_eq!(peak, 5);
        let energy: f64 = pattern.iter().map(|value| value * value).sum();
        assert!((correlation[5] - energy).abs() < 1e-9);
    }

    #[test]
    fn short_and_empty_buffers_are_accepted() {
        let mut single = [Complex::real(4.0)];
        transform(&mut single, Direction::Forward);
        assert_eq!(single[0], Complex::real(4.0));

        assert_eq!(forward_real(&[]), vec![Complex::default()]);
        assert_eq!(magnitude_spectrum(&[]).len(), 1);
    }

    #[test]
    #[should_panic(expected = "not a power of two")]
    fn non_power_of_two_lengths_are_rejected() {
        let mut buffer = vec![Complex::default(); 6];
        transform(&mut buffer, Direction::Forward);
    }
}
