use othello_complexity_rs::othello::CENTER_MASK;
use othello_complexity_rs::prunings::occupancy::{occupancy_order, occupancy_order_naive};
use rand::{Rng, SeedableRng};
use std::time::Instant;

fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "true" | "True" | "TRUE" | "yes" | "Yes" | "YES" => Some(true),
        "0" | "false" | "False" | "FALSE" | "no" | "No" | "NO" => Some(false),
        _ => None,
    }
}

fn fold_checksum(mut acc: u64, values: &[u64; 64]) -> u64 {
    for &v in values {
        acc = acc.rotate_left(7) ^ v.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    acc
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let samples: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let with_center: bool = args.get(2).and_then(|s| parse_bool(s)).unwrap_or(true);
    let seed: u64 = args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x0cc5eedu64);

    let mut boards = Vec::with_capacity(samples);
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    for _ in 0..samples {
        let mut occupied = rng.random::<u64>();
        if with_center {
            occupied |= CENTER_MASK;
        }
        boards.push(occupied);
    }

    // Correctness check before timing.
    for &occupied in &boards {
        let naive = occupancy_order_naive(occupied);
        let bitparallel = occupancy_order(occupied);
        assert_eq!(
            naive, bitparallel,
            "mismatch naive vs bitparallel occupied={:#018x}, with_center={}",
            occupied, with_center
        );
    }

    let mut naive_checksum = 0u64;

    let start = Instant::now();
    for &occupied in &boards {
        let out = occupancy_order_naive(occupied);
        naive_checksum = fold_checksum(naive_checksum, &out);
    }
    let naive_elapsed = start.elapsed();

    let mut bitparallel_checksum = 0u64;
    let start = Instant::now();
    for &occupied in &boards {
        let out = occupancy_order(occupied);
        bitparallel_checksum = fold_checksum(bitparallel_checksum, &out);
    }
    let bitparallel_elapsed = start.elapsed();

    let naive_ns = naive_elapsed.as_nanos() as f64 / samples as f64;
    let bitparallel_ns = bitparallel_elapsed.as_nanos() as f64 / samples as f64;
    let speedup_naive_vs_bitparallel =
        naive_elapsed.as_secs_f64() / bitparallel_elapsed.as_secs_f64();

    println!(
        "samples={}, with_center={}, seed={}",
        samples, with_center, seed
    );
    println!("naive total={:?}, {:.1} ns/board", naive_elapsed, naive_ns);
    println!(
        "bitparallel total={:?}, {:.1} ns/board",
        bitparallel_elapsed, bitparallel_ns
    );
    println!(
        "speedup naive/bitparallel={:.3}x",
        speedup_naive_vs_bitparallel,
    );
    println!("naive checksum={:#018x}", naive_checksum);
    println!("bitparallel checksum={:#018x}", bitparallel_checksum);
}
