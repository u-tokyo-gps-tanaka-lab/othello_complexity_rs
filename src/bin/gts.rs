use clap::Parser;
use rand::rngs::StdRng;
use rand::Rng;
use rand::SeedableRng;
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;

use othello_complexity_rs::othello::{flip, get_moves, Board};

/// Monte‑Carlo estimator of game tree size (gts) for Othello.
///
/// A single random play `g` samples uniformly from legal moves (passes are
/// treated as a forced single move). For that play, let `c_j` be the number of
/// legal moves on ply `j`; the path weight is `X(g) = Π_j c_j`.  The expected
/// value of `X(g)` over all random plays equals the game‑tree size, so we
/// approximate gts by the sample mean of `X(g)`.
#[derive(Debug, Parser)]
#[command(
    name = "gts",
    about = "Estimate Othello game tree size via random playouts"
)]
struct Cli {
    /// Number of random playouts per trial (n)
    #[arg(short = 'n', long, default_value_t = 1_000_000)]
    playouts: usize,

    /// Number of independent trials to run (t)
    #[arg(short = 't', long, default_value_t = 1)]
    trials: usize,

    /// Base RNG seed (optional; enables reproducible runs)
    #[arg(long)]
    seed: Option<u64>,

    /// Print progress every k playouts inside each trial (0 = silent)
    #[arg(short = 'p', long, default_value_t = 0)]
    progress_every: usize,
}

/// Run one random self‑play from the initial position.
/// Returns (X(g), plies played).
fn random_play_weight<R: Rng + ?Sized>(rng: &mut R) -> (f64, u32) {
    let mut b = Board::initial();
    let mut weight: f64 = 1.0;
    let mut plies: u32 = 0;

    loop {
        let moves = get_moves(b.player, b.opponent);
        if moves == 0 {
            // pass if opponent has a move; otherwise game ends
            let opp_moves = get_moves(b.opponent, b.player);
            if opp_moves == 0 {
                break;
            }
            // forced pass counts as single branch (c_j = 1)
            b = Board::new(b.opponent, b.player);
            plies += 1;
            continue;
        }

        let cnt = moves.count_ones();
        weight *= cnt as f64;
        plies += 1;

        // select k-th set bit at random
        let r = rng.random_range(0..cnt);
        let mut m = moves;
        let mut idx = 0;
        for _ in 0..=r {
            idx = m.trailing_zeros();
            m &= m - 1;
        }

        let flipped = flip(idx as usize, b.player, b.opponent);
        debug_assert!(flipped != 0);

        // apply move: next side becomes previous opponent
        b = Board {
            player: b.opponent ^ flipped,
            opponent: b.player ^ (flipped | (1u64 << idx)),
        };
    }

    (weight, plies)
}

#[inline]
fn mix_u64(mut x: u64) -> u64 {
    // SplitMix64
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn estimate_trial_parallel(playouts: usize, base_seed: u64, trial_id: usize) -> (f64, f64) {
    let threads = rayon::current_num_threads().max(1);
    let chunks_per_thread = 16usize;
    let denom = threads.saturating_mul(chunks_per_thread).max(1);
    let mut chunk_size = (playouts + denom - 1) / denom;
    chunk_size = chunk_size.clamp(512, 32_768);
    let num_chunks = (playouts + chunk_size - 1) / chunk_size;

    let (sum, sum_sq, count) = (0..num_chunks)
        .into_par_iter()
        .map(|chunk_id| {
            let start = chunk_id * chunk_size;
            let end = ((chunk_id + 1) * chunk_size).min(playouts);
            let len = end - start;

            let seed_material = base_seed
                ^ (trial_id as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93)
                ^ (chunk_id as u64).wrapping_mul(0xA5A3_56A6_5C2F_5B5D);
            let mut rng = StdRng::seed_from_u64(mix_u64(seed_material));

            let mut local_sum = 0.0f64;
            let mut local_sum_sq = 0.0f64;
            for _ in 0..len {
                let (w, _) = random_play_weight(&mut rng);
                local_sum += w;
                local_sum_sq += w * w;
            }
            (local_sum, local_sum_sq, len as u64)
        })
        .reduce(
            || (0.0f64, 0.0f64, 0u64),
            |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2),
        );

    debug_assert_eq!(count as usize, playouts);
    let n = count as f64;
    let mean = sum / n;
    let var = (sum_sq / n) - mean * mean;
    let stderr = (var.max(0.0) / n).sqrt();
    (mean, stderr)
}

fn estimate_trial_sequential(playouts: usize, progress_every: usize) -> (f64, f64) {
    let mut rng = rand::rng();
    let mut sum = 0.0f64;
    let mut sum_sq = 0.0f64;

    for i in 1..=playouts {
        let (w, _) = random_play_weight(&mut rng);
        sum += w;
        sum_sq += w * w;

        if progress_every != 0 && i % progress_every == 0 {
            let mean = sum / i as f64;
            println!("sample {:>8}: mean = {:.6e}", i, mean);
        }
    }

    let n = playouts as f64;
    let mean = sum / n;
    let var = (sum_sq / n) - mean * mean;
    let stderr = (var.max(0.0) / n).sqrt();
    (mean, stderr)
}

fn main() {
    let args = Cli::parse();
    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    if std::env::var("RAYON_NUM_THREADS").is_err() {
        let _ = ThreadPoolBuilder::new()
            .num_threads(available)
            .build_global();
    }
    let rayon_threads = rayon::current_num_threads();
    println!(
        "threads      : rayon = {}, available_parallelism = {}",
        rayon_threads, available
    );

    let mut seed_rng = rand::rng();
    let base_seed = args.seed.unwrap_or_else(|| seed_rng.random());
    if args.seed.is_some() {
        println!("seed         : {}", base_seed);
    }

    let mut trial_means = Vec::with_capacity(args.trials);

    for t in 1..=args.trials {
        let (mean, stderr) = if args.progress_every != 0 {
            println!("trial {:>4}: sequential (progress enabled)", t);
            estimate_trial_sequential(args.playouts, args.progress_every)
        } else {
            estimate_trial_parallel(args.playouts, base_seed, t)
        };

        println!(
            "trial {:>4}: playouts = {:>8}, mean = {:.6e}, stderr = {:.6e}",
            t, args.playouts, mean, stderr
        );

        trial_means.push(mean);
    }

    if args.trials > 1 {
        let t = args.trials as f64;
        let sum_means: f64 = trial_means.iter().sum();
        let mean_of_means = sum_means / t;
        let var_between = trial_means
            .iter()
            .map(|m| (m - mean_of_means).powi(2))
            .sum::<f64>()
            / t;
        let sd_between = var_between.sqrt();
        let stderr_between = sd_between / t.sqrt();

        println!("--- summary over trials (trial means) ---");
        println!("trials                : {}", args.trials);
        println!("mean of trial means   : {:.6e}", mean_of_means);
        println!("SD of trial means     : {:.6e}", sd_between);
        println!("SE of mean (over trial means): {:.6e}", stderr_between);
    }
}
