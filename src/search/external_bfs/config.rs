use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(name = "reverse_to_initial_bfs", version)]
pub struct Cfg {
    /// 入力ファイル
    pub input: PathBuf,

    /// 出力ディレクトリ
    #[arg(short, long, default_value = "result")]
    pub out_dir: PathBuf,

    /// スレッド数（0で自動）
    #[arg(short = 'j', long, default_value_t = 0)]
    pub jobs: usize,

    /// ログ詳細度
    #[arg(short, long, default_value_t = 0)]
    pub verbose: u8,

    /// ブロックサイズ
    #[arg(short = 'b', long, default_value_t = 1000000)]
    pub block_size: usize,

    /// forwardとreverseで合流する石数
    #[arg(short = 'd', long, default_value_t = 10)]
    pub discs: usize,

    /// tmp_dir
    #[arg(short = 't', long, default_value = "tmp")]
    pub tmp_dir: PathBuf,

    /// resume
    #[arg(short = 'r', long)]
    pub resume: bool,
}
