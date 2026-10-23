use std::path::PathBuf;

/// External BFS persists exact queries and the settings below. Resume uses the
/// same input file and temporary directory; any changed query or setting fails.
#[derive(Debug, Clone)]
pub struct Cfg {
    pub input: PathBuf,
    pub out_dir: PathBuf,
    pub jobs: usize,
    pub verbose: u8,
    pub block_size: usize,
    pub discs: usize,
    pub tmp_dir: PathBuf,
    pub resume: bool,
}
