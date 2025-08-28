pub mod lib {
    pub mod othello;
    pub mod reverse_search;
    pub mod solve_kissat;
}

const MASKS: [u64; 4] = [
    0x7e7e7e7e7e7e7e7e, // horizontal
    0xffffffffffffff00, // vertical
    0x7e7e7e7e7e7e7e00, // diag \
    0x007e7e7e7e7e7e7e, // diag /
];

#[inline]
fn bit_scan_forward64(mask: u64) -> Option<u32> {
    if mask == 0 {
        None
    } else {
        Some(mask.trailing_zeros())
    }
}

fn mask_to_moves(m: u64) -> String {
    let mut ans: Vec<String> = vec!["[".to_string()];
    for i in 0..64 {
        if m & (1 << i) != 0 {
            let y = i / 8;
            let x = i % 8;
            ans.push(format!("({}, {})", x, y));
        }
    }
    ans.push("]".to_string());
    ans.join(",")
}
