pub mod lib {
    pub mod io;
    pub mod othello;
    pub mod search;
    pub mod solve_kissat;
}

const MASKS: [u64; 4] = [
    0x7e7e7e7e7e7e7e7e, // horizontal
    0xffffffffffffff00, // vertical
    0x7e7e7e7e7e7e7e00, // diag \
    0x007e7e7e7e7e7e7e, // diag /
];
