use std::fmt;
use std::io;
use std::num::NonZeroU128;

use bigdecimal::{BigDecimal, FromPrimitive};
use statrs::function::erf::erfc_inv;

#[derive(Debug)]
pub struct Interval {
    pub population_size: NonZeroU128,
    pub sample_size: u64,
    pub observed_lower: f64,
    pub observed_upper: f64,
    pub lower: f64,
    pub upper: f64,
    pub expected_lower: BigDecimal,
    pub expected_upper: BigDecimal,
    confidence_level: f64,
}

pub fn compute(
    ok: u64,
    ng: u64,
    unknown: u64,
    population_size: NonZeroU128,
    alpha: f64,
) -> io::Result<Interval> {
    let sample_size = ok
        .checked_add(ng)
        .and_then(|n| n.checked_add(unknown))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Sample count overflow: ok + ng + unknown exceeds u64.",
            )
        })?;
    if sample_size == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Sample size N = ok + ng + unknown must be > 0.",
        ));
    }
    validate_alpha(alpha)?;

    let n = sample_size as f64;
    // erfc avoids rounding 1 - alpha/2 to 1 for very small finite alpha.
    let z = std::f64::consts::SQRT_2 * erfc_inv(alpha);
    let lower = wilson_bounds(ok as f64, n, z).0;
    let upper = wilson_bounds((ok + unknown) as f64, n, z).1;
    let population = BigDecimal::from(population_size.get());
    let expected_lower = BigDecimal::from_f64(lower)
        .ok_or_else(|| io::Error::other("failed to convert lower bound to BigDecimal"))?
        * &population;
    let expected_upper = BigDecimal::from_f64(upper)
        .ok_or_else(|| io::Error::other("failed to convert upper bound to BigDecimal"))?
        * &population;

    Ok(Interval {
        population_size,
        sample_size,
        observed_lower: ok as f64 / n,
        observed_upper: (ok + unknown) as f64 / n,
        lower,
        upper,
        expected_lower,
        expected_upper,
        confidence_level: 100.0 * (1.0 - alpha),
    })
}

pub(crate) fn validate_alpha(alpha: f64) -> io::Result<()> {
    if alpha.is_finite() && 0.0 < alpha && alpha < 1.0 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "alpha must be finite and in (0,1).",
        ))
    }
}

impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Population size = {}", self.population_size)?;
        writeln!(f, "Sample size = {}", self.sample_size)?;
        writeln!(
            f,
            "Observed reachable fraction range: [{:.12}, {:.12}]",
            self.observed_lower, self.observed_upper
        )?;
        writeln!(
            f,
            "{}% approximate Wilson CI: [{:.12}, {:.12}]",
            self.confidence_level, self.lower, self.upper
        )?;
        write!(
            f,
            "Expected |R| interval: [{:.6e}, {:.6e}]",
            self.expected_lower, self.expected_upper
        )
    }
}

/// source:
/// - https://en.wikipedia.org/wiki/Binomial_proportion_confidence_interval#Wilson_score_interval
/// - https://www.itl.nist.gov/div898/handbook/prc/section2/prc241.htm
fn wilson_bounds(x: f64, n: f64, z: f64) -> (f64, f64) {
    let p_hat = x / n;
    let z2 = z * z;
    let denom = 1.0 + z2 / n;
    let center = p_hat + z2 / (2.0 * n);
    let rad = z * ((p_hat * (1.0 - p_hat)) / n + z2 / (4.0 * n * n)).sqrt();
    let lower = if x == 0.0 {
        0.0
    } else {
        (center - rad) / denom
    };
    let upper = if x == n { 1.0 } else { (center + rad) / denom };
    (lower, upper)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigdecimal::ToPrimitive;

    // P(|Z| > 1), from the standard normal integral. At this alpha the
    // independent Wilson-score quadratic has z² = 1.
    const ONE_SIGMA_ALPHA: f64 = 0.31731050786291415;

    fn compute(ok: u64, ng: u64, unknown: u64) -> (f64, f64, f64) {
        let interval = super::compute(
            ok,
            ng,
            unknown,
            NonZeroU128::new(1).unwrap(),
            ONE_SIGMA_ALPHA,
        )
        .unwrap();
        (interval.lower, interval.upper, interval.confidence_level)
    }

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    #[test]
    fn extreme_outcomes_obey_the_score_quadratic() {
        let all_ok = compute(10, 0, 0);
        close(all_ok.0, 10.0 / 11.0);
        assert_eq!(all_ok.1, 1.0);
        let all_ng = compute(0, 10, 0);
        assert_eq!(all_ng.0, 0.0);
        close(all_ng.1, 1.0 / 11.0);
        let all_unknown = compute(0, 0, 10);
        assert_eq!((all_unknown.0, all_unknown.1), (0.0, 1.0));
    }

    #[test]
    fn known_outcomes_and_unknown_envelope_use_the_same_denominator() {
        let known = compute(5, 5, 0);
        close(known.0, 0.5 - 1.0 / (44.0_f64).sqrt());
        close(known.1, 0.5 + 1.0 / (44.0_f64).sqrt());
        let envelope = compute(3, 5, 2);
        close(envelope.0, compute(3, 7, 0).0);
        close(envelope.1, known.1);
    }

    #[test]
    fn invalid_counts_and_significance_are_errors() {
        let population_size = NonZeroU128::new(1).unwrap();
        for counts in [(0, 0, 0), (u64::MAX, 1, 0), (0, u64::MAX, 1)] {
            assert!(super::compute(counts.0, counts.1, counts.2, population_size, 0.005).is_err());
        }
        for alpha in [0.0, 1.0, -0.1, f64::NAN, f64::INFINITY] {
            assert!(super::compute(1, 0, 0, population_size, alpha).is_err());
        }
    }

    #[test]
    fn even_the_smallest_positive_alpha_has_finite_bounds() {
        let interval =
            super::compute(1, 1, 0, NonZeroU128::new(1).unwrap(), f64::from_bits(1)).unwrap();
        let (lower, upper) = (interval.lower, interval.upper);
        assert!(lower.is_finite() && upper.is_finite());
        assert!(0.0 <= lower && lower <= upper && upper <= 1.0);
    }

    #[test]
    fn sample_counts_and_population_size_scale_the_interval() {
        // With ten unknown observations, the interval covers the entire
        // population. This also checks the exact u128 population.
        let population_size = NonZeroU128::new(u128::MAX).unwrap();
        let interval = super::compute(0, 0, 10, population_size, ONE_SIGMA_ALPHA).unwrap();
        assert_eq!(interval.sample_size, 10);
        assert_eq!(interval.expected_lower, BigDecimal::from(0));
        assert_eq!(interval.expected_upper, BigDecimal::from(u128::MAX));

        // With one success out of two observations and z² = 1, the score
        // quadratic gives 1/2 ± 1/(2√3), scaled here by a population of 100.
        let interval =
            super::compute(1, 1, 0, NonZeroU128::new(100).unwrap(), ONE_SIGMA_ALPHA).unwrap();
        close(interval.lower, 0.5 - 0.5 / 3.0_f64.sqrt());
        close(interval.upper, 0.5 + 0.5 / 3.0_f64.sqrt());
        close(
            interval.expected_lower.to_f64().unwrap(),
            50.0 - 50.0 / 3.0_f64.sqrt(),
        );
        close(
            interval.expected_upper.to_f64().unwrap(),
            50.0 + 50.0 / 3.0_f64.sqrt(),
        );
    }

    #[test]
    fn display_keeps_compute_ci_output_format() {
        let interval =
            super::compute(0, 0, 10, NonZeroU128::new(100).unwrap(), ONE_SIGMA_ALPHA).unwrap();
        assert_eq!(
            interval.to_string(),
            concat!(
                "Population size = 100\n",
                "Sample size = 10\n",
                "Observed reachable fraction range: [0.000000000000, 1.000000000000]\n",
                "68.26894921370858% approximate Wilson CI: [0.000000000000, 1.000000000000]\n",
                "Expected |R| interval: [0.000000e+0, 1.000000e+2]"
            )
        );
    }
}
