use bigdecimal::{BigDecimal, FromPrimitive};
use clap::Parser;
use statrs::distribution::{ContinuousCDF, Normal};
use std::error::Error;

const POPULATION_SIZE: u128 = 3_u128.pow(60) * 2_u128.pow(4);

#[derive(Debug, Parser)]
#[command(
    name = "compute-ci",
    about = "Compute Wilson score confidence interval bounds"
)]
struct Args {
    /// Count of observed successes
    #[arg(long)]
    ok: u64,

    /// Count of observed failures
    #[arg(long)]
    ng: u64,

    /// Count of samples with unknown outcome
    #[arg(long)]
    unknown: u64,

    /// Significance level (two-sided alpha); e.g. 0.05 for 95% CI
    #[arg(long, default_value_t = 0.05)]
    alpha: f64,
}

#[derive(Debug)]
struct WilsonCI {
    ok: u64,
    ng: u64,
    unknown: u64,
    alpha: f64,
}

impl WilsonCI {
    fn compute(&self) -> Result<(f64, f64, f64), Box<dyn Error>> {
        self.validate()?;
        let n = (self.ok + self.ng + self.unknown) as f64;

        let normal = Normal::new(0.0, 1.0).unwrap();
        let z = normal.inverse_cdf(1.0 - self.alpha / 2.0);

        let lower = wilson_lower(self.ok as f64, n, z);
        let upper = wilson_upper((self.ok + self.unknown) as f64, n, z);
        Ok((lower, upper, 1.0 - self.alpha))
    }

    fn validate(&self) -> Result<(), Box<dyn Error>> {
        if self.ok == 0 && self.ng == 0 && self.unknown == 0 {
            return Err("Sample size N = ok + ng + unknown must be > 0.".into());
        }
        if self.ok + self.unknown > self.ok + self.ng + self.unknown {
            return Err("Counts inconsistent: require ok+unknown ≤ N.".into());
        }
        if self.alpha <= 0.0 || self.alpha >= 1.0 {
            return Err("alpha must be in (0,1).".into());
        }
        Ok(())
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
    let lower = (center - rad) / denom;
    let upper = (center + rad) / denom;
    (lower, upper)
}

fn wilson_lower(x: f64, n: f64, z: f64) -> f64 {
    wilson_bounds(x, n, z).0
}

fn wilson_upper(x: f64, n: f64, z: f64) -> f64 {
    wilson_bounds(x, n, z).1
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let (lower, upper, conf_level) = WilsonCI {
        ok: args.ok,
        ng: args.ng,
        unknown: args.unknown,
        alpha: args.alpha,
    }
    .compute()?;

    let population = BigDecimal::from(POPULATION_SIZE);
    let expected_lower = BigDecimal::from_f64(lower)
        .ok_or("failed to convert lower bound to BigDecimal")?
        * &population;
    let expected_upper = BigDecimal::from_f64(upper)
        .ok_or("failed to convert upper bound to BigDecimal")?
        * &population;

    println!(
        "{}% Wilson CI: [{:.6}, {:.6}]",
        100.0 * conf_level,
        lower,
        upper
    );
    println!(
        "Expected |R| interval: [{:.6e}, {:.6e}]",
        expected_lower, expected_upper
    );
    Ok(())
}
