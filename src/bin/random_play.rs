use clap::Parser;
use rand::Rng;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use othello_complexity_rs::othello::{flip, get_moves, Board};

#[derive(Parser, Debug)]
#[command(
    name = "random_play",
    about = "Generate random Othello games from initial position"
)]
struct Args {
    /// ランダムに指す手数 (0ならば手数もランダム)
    #[arg(short = 'n', long, default_value_t = 0)]
    stone_count: usize,

    /// 生成個数
    #[arg(short = 'c', long, default_value_t = 50)]
    gen_count: usize,

    /// 出力先ディレクトリ
    #[arg(short = 'o', long = "out-dir", value_name = "DIR")]
    out_dir: Option<PathBuf>,
}

/// ビット位置 (0-63) を先手視点の座標文字列 (a1-h8) に変換
#[inline]
fn idx_to_coord(idx: usize) -> String {
    let x = idx % 8;
    let y = idx / 8;
    let file = (b'a' + x as u8) as char;
    format!("{}{}", file, y + 1)
}

/// 初期局面から nmoves 手ランダムに指した局面とその棋譜を返す
/// nmoves == 0 の場合は手数自体もランダム（石5〜64個埋まり = 1〜60手）
/// 戻り値の bool は「最終局面で手番側(player)が黒かどうか」を表す
fn do_random_play(nmoves: usize) -> (Board, String) {
    let mut rng = rand::rng();
    let mut b = Board::initial();
    let mut record = String::new();
    let mut is_player_black = true;
    let max_moves = if nmoves == 0 {
        rng.random_range(1..=60)
    } else {
        nmoves
    };

    let mut moves_played = 0;
    while moves_played < max_moves {
        let mut m = get_moves(b.player, b.opponent);
        if m == 0 {
            let m1 = get_moves(b.opponent, b.player);
            if m1 == 0 {
                break; // 両者パス → ゲーム終了
            }
            b = Board::new(b.opponent, b.player);
            is_player_black = !is_player_black;
            m = m1;
        }
        let cnt = m.count_ones();
        let r = rng.random_range(0..cnt);
        let mut idx = 0;
        for _ in 0..=r {
            idx = m.trailing_zeros();
            m &= m - 1;
        }
        let flipped = flip(idx as usize, b.player, b.opponent);
        if flipped == 0 {
            continue;
        }
        record.push_str(&idx_to_coord(idx as usize));
        b = Board {
            player: b.opponent ^ flipped,
            opponent: b.player ^ (flipped | (1u64 << idx)),
        };
        is_player_black = !is_player_black;
        moves_played += 1;
    }
    (b, record)
}

fn main() -> std::io::Result<()> {
    let args = Args::parse();

    let out_dir = args.out_dir.unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("result")
            .join("random_play")
    });
    if !out_dir.exists() {
        fs::create_dir_all(&out_dir)?;
    }

    let position_path = out_dir.join(format!(
        "random_play_n{}_c{}.txt",
        args.stone_count, args.gen_count
    ));
    let record_path = out_dir.join(format!(
        "random_play_n{}_c{}_record.txt",
        args.stone_count, args.gen_count
    ));
    let mut position_file = File::create(&position_path)?;
    let mut record_file = File::create(&record_path)?;
    for _ in 0..args.gen_count {
        let (b, record) = do_random_play(args.stone_count);
        writeln!(position_file, "{}", b.to_string())?;
        writeln!(record_file, "{record}")?;
    }
    Ok(())
}
