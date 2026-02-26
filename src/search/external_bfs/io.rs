use std::fs::File;
use std::io::{self, BufReader, BufWriter, Error, ErrorKind, Read, Result, Write};
use std::path::PathBuf;

use bytemuck;

use crate::othello::{get_moves, Board};
use crate::search::core::SearchResult;

/// ファイルサイズが16バイトの倍数であることを検証
///
/// # パラメータ
/// - `len`: 検証するファイルサイズ(バイト単位)
/// - `context`: エラーメッセージに含めるコンテキスト文字列
///
/// # 戻り値
/// ファイルサイズが16の倍数であれば`Ok(())`、そうでなければ`InvalidData`エラー
///
/// # アルゴリズム詳細
/// オセロ盤面は[u64; 2]で表現され、1レコードが16バイト固定であるため、
/// ファイルサイズが16の倍数でない場合はデータ破損とみなす。
pub(in crate::search::external_bfs) fn validate_file_size(len: u64, context: &str) -> Result<()> {
    if len % 16 != 0 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!(
                "{}: file size {} is not a multiple of 16 bytes",
                context, len
            ),
        ));
    }
    Ok(())
}

/// 探索すべき盤面をファイルに書き出す
///
/// # パラメータ
/// - `tmp_dir`: 一時ディレクトリのパス
/// - `board`: 書き出す盤面
/// - `num_disc`: 石数(ファイル名の命名に使用)
///
/// # 戻り値
/// 書き込み成功時は`Ok(())`、I/Oエラー時は`Err`
///
/// # アルゴリズム詳細
/// - 盤面を[player, opponent]の形式で保存
/// - パス判定を行い、相手にパスが必要な場合は盤面を反転して追加保存
/// - 出力ファイル名は`r_{num_disc}.bin`形式
pub(in crate::search::external_bfs) fn write_given_board(
    tmp_dir: &PathBuf,
    board: &Board,
    num_disc: usize,
) -> Result<()> {
    let mut boards: Vec<[u64; 2]> = vec![[board.player, board.opponent]];
    if get_moves(board.opponent, board.player) == 0 {
        boards.push([board.opponent, board.player]);
    }
    let rfilename = format!("r_{}.bin", num_disc);
    let rfile = File::create(&tmp_dir.join(rfilename))?;
    let mut w = BufWriter::new(rfile);
    w.write_all(bytemuck::cast_slice(&boards))?;
    w.flush()?;
    Ok(())
}

/// 探索対象の盤面をファイルに書き出す(resume用)
///
/// # パラメータ
/// - `tmp_dir`: 一時ディレクトリのパス
/// - `board`: 書き出す盤面
///
/// # 戻り値
/// 書き込み成功時は`Ok(())`、I/Oエラー時は`Err`
///
/// # アルゴリズム詳細
/// - 盤面を[player, opponent]の形式で`target_board.bin`に保存
/// - resume時に`read_target_board`で読み込み、`make_fwd_table`に渡す
pub(in crate::search::external_bfs) fn write_target_board(
    tmp_dir: &PathBuf,
    board: &Board,
) -> Result<()> {
    let bytes: [u64; 2] = [board.player, board.opponent];
    let file = File::create(&tmp_dir.join("target_board.bin"))?;
    let mut w = BufWriter::new(file);
    w.write_all(bytemuck::cast_slice(&bytes))?;
    w.flush()
}

/// resume時に探索対象の盤面を読み込む
///
/// # パラメータ
/// - `tmp_dir`: 一時ディレクトリのパス
///
/// # 戻り値
/// 盤面の`[player, opponent]`配列、またはI/Oエラー
///
/// # アルゴリズム詳細
/// - `write_target_board`で保存された`target_board.bin`を読み込む
/// - ネイティブエンディアンで解釈
pub fn read_target_board(tmp_dir: &PathBuf) -> Result<[u64; 2]> {
    let file = File::open(&tmp_dir.join("target_board.bin"))?;
    let mut r = BufReader::new(file);
    let mut buf = [0u8; 16];
    r.read_exact(&mut buf)?;
    let p = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
    let o = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
    Ok([p, o])
}

/// 最終結果ファイルをリーフノードと照合
///
/// # パラメータ
/// - `tmp_dir`: 一時ディレクトリのパス
/// - `discs`: 目標石数
/// - `leafnode`: 前進探索で生成された到達可能盤面のソート済みVec
///
/// # 戻り値
/// リーフノードに一致する盤面が見つかれば`SearchResult::Found`、
/// 全レコードを確認して一致がなければ`SearchResult::NotFound`
///
/// # アルゴリズム詳細
/// - 後退探索の最終層(`r_{discs}.bin`)を読み込み
/// - 各レコードをネイティブエンディアンで解釈し、リーフノードとの一致判定を行う
/// - 一致した時点で即座に`Found`を返す(早期終了)
/// - ソート済みVecに対してbinary_searchを使用
pub(in crate::search::external_bfs) fn check_leafnode_match(
    tmp_dir: &PathBuf,
    discs: i32,
    leafnode: &Vec<[u64; 2]>,
) -> Result<SearchResult> {
    let rfilename = format!("r_{}.bin", discs);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let len = file.metadata()?.len();
    validate_file_size(len, "check_leafnode_match")?;

    let mut r = BufReader::new(file);
    let mut buf = [0u8; 16];
    let nrecs = len / 16;

    for _ in 0..nrecs {
        r.read_exact(&mut buf)?;
        let a = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
        let b = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
        let uni = [a, b];
        if leafnode.binary_search(&uni).is_ok() {
            return Ok(SearchResult::Found);
        }
    }
    Ok(SearchResult::NotFound)
}

/// 1レコード(=16バイト)をネイティブエンディアンのまま読み取る
///
/// # パラメータ
/// - `reader`: 読み込み元のバッファ付きファイルリーダ
///
/// # 戻り値
/// - `Ok(Some((p, o)))`: player/opponentのu64ペアを読み込んだ場合
/// - `Ok(None)`: EOFに到達した場合
/// - `Err`: 部分読み込みエラーや不完全なレコード
///
/// # アルゴリズム詳細
/// - 最初に1バイト読み込んでEOF判定を行い、部分読み込みエラーを回避
/// - ネイティブエンディアンを使用してプラットフォーム依存の高速化を実現
/// - バイナリフォーマット: \[player:8bytes\]\[opponent:8bytes\]
pub(crate) fn read_pair(reader: &mut BufReader<File>) -> io::Result<Option<(u64, u64)>> {
    let mut buf = [0u8; 16];
    // まず 1 バイト読んで EOF 判定を分ける（partial read 対策）
    match reader.read(&mut buf[..1])? {
        0 => return Ok(None), // EOF
        1 => {
            // すでに 1 バイト読んだので残り 15 バイト読む
            reader.read_exact(&mut buf[1..])?;
        }
        _ => unreachable!(),
    }
    let p = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
    let o = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
    Ok(Some((p, o)))
}

/// 1レコードを書き出し(ネイティブエンディアンのまま)
///
/// # パラメータ
/// - `writer`: 書き込み先のバッファ付きファイルライタ
/// - `p`: playerのビットボード(u64)
/// - `o`: opponentのビットボード(u64)
///
/// # 戻り値
/// 書き込み成功時は`Ok(())`、I/Oエラー時は`Err`
///
/// # アルゴリズム詳細
/// - ネイティブエンディアンで16バイト書き込み
/// - read_pairと対称的な実装により、同一プラットフォーム内での入出力を保証
pub(crate) fn write_pair(writer: &mut BufWriter<File>, p: u64, o: u64) -> io::Result<()> {
    writer.write_all(&p.to_ne_bytes())?;
    writer.write_all(&o.to_ne_bytes())?;
    Ok(())
}
