use crate::othello::{
    coord_to_square, square_to_coord, validate_board, Board, BoardSymmetry, Color, Position,
};
use crate::record::{GoalMatch, PlyAction, ReachabilityRecord};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::str::FromStr;

/// `64文字のX/O/- + 半角空白 + b/w` を相対盤面と絶対手番へ変換する。
///
/// `X` は常に手番側であり、末尾の色が `X` の絶対色を表す。
/// 手番の省略、余分なフィールド、不正な文字と空の中央マスを拒否する。
pub fn parse_position(line: &str) -> Result<Position, String> {
    let bytes = line.as_bytes();
    if bytes.len() != 66 || bytes[64] != b' ' {
        return Err("expected exactly 64 X/O/- cells, one space, and b or w".to_owned());
    }
    let side_to_move = match bytes[65] {
        b'b' => Color::Black,
        b'w' => Color::White,
        _ => return Err("side to move must be b or w".to_owned()),
    };
    let mut player = 0_u64;
    let mut opponent = 0_u64;
    for (square, &cell) in bytes[..64].iter().enumerate() {
        match cell {
            b'X' => player |= 1_u64 << square,
            b'O' => opponent |= 1_u64 << square,
            b'-' => (),
            _ => return Err(format!("invalid board character at cell {}", square + 1)),
        }
    }
    let board = Board::new(player, opponent);
    validate_board(&board).map_err(|error| format!("invalid board: {error:?}"))?;
    Ok(Position {
        board,
        side_to_move,
    })
}

/// 相対盤面と絶対手番を `64文字のX/O/- + 半角空白 + b/w` で保存する。
pub fn format_position(position: Position) -> String {
    let color = match position.side_to_move {
        Color::Black => 'b',
        Color::White => 'w',
    };
    format!("{} {color}", position.board.to_string())
}

/// 1行1局面のファイルを読む。不正行は空白行も含めて行番号付きエラーとする。
///
/// 局面の順序と重複を保存し、空ファイルは空の中間分類として受理する。
pub fn parse_file_to_positions(path: &str) -> io::Result<Vec<Position>> {
    let reader = BufReader::new(File::open(path)?);
    let mut positions = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|error| {
            io::Error::new(error.kind(), format!("line {}: {error}", index + 1))
        })?;
        let position = parse_position(&line).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("line {}: {error}", index + 1),
            )
        })?;
        positions.push(position);
    }
    Ok(positions)
}

impl FromStr for BoardSymmetry {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|symmetry| symmetry.as_str() == value)
            .ok_or_else(|| format!("unknown board symmetry: {value}"))
    }
}

impl FromStr for GoalMatch {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "exact" => Ok(Self::Exact),
            "symmetry" => Ok(Self::SymmetryOrbit),
            "equivalence" => Ok(Self::EquivalenceClass),
            _ => Err(format!("unknown reachability mode: {value}")),
        }
    }
}

/// 半角空白で区切られた `A1`〜`H8` と `ps` を読む。空文字列は長さ0の手順。
pub fn parse_actions(record: &str) -> Result<Vec<PlyAction>, String> {
    if record.is_empty() {
        return Ok(Vec::new());
    }
    record
        .split(' ')
        .enumerate()
        .map(|(index, token)| {
            if token == "ps" {
                return Ok(PlyAction::Pass);
            }
            let bytes = token.as_bytes();
            if bytes.len() != 2 || !(b'A'..=b'H').contains(&bytes[0]) {
                return Err(format!(
                    "action {}: expected A1 through H8 or ps",
                    index + 1
                ));
            }
            coord_to_square(token)
                .map(PlyAction::Play)
                .map_err(|error| format!("action {}: {error}", index + 1))
        })
        .collect()
}

/// 全着手と全パスを半角空白で区切る。空手順は空文字列となる。
pub fn format_actions(actions: &[PlyAction]) -> String {
    actions
        .iter()
        .map(|action| match action {
            PlyAction::Play(square) => square_to_coord(*square),
            PlyAction::Pass => "ps".to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn format_reachability_record(record: &ReachabilityRecord) -> String {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    writer
        .write_record([
            record.input_index.to_string(),
            format_position(record.start),
            format_position(record.goal),
            record.goal_match.to_string(),
            record.transform.to_string(),
            format_actions(&record.actions),
        ])
        .expect("writing one CSV record to memory cannot fail");
    let mut bytes = writer
        .into_inner()
        .expect("flushing a CSV record to memory cannot fail");
    assert_eq!(bytes.pop(), Some(b'\n'), "CSV record must end in a newline");
    String::from_utf8(bytes).expect("CSV record fields are valid UTF-8")
}

pub(crate) fn parse_reachability_record(text: &str) -> Result<ReachabilityRecord, String> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(text.as_bytes());
    let mut records = reader.records();
    let fields = records
        .next()
        .ok_or("expected one CSV record")?
        .map_err(|error| format!("invalid CSV record: {error}"))?;
    if records
        .next()
        .transpose()
        .map_err(|error| format!("invalid CSV record: {error}"))?
        .is_some()
    {
        return Err("expected one CSV record, found multiple records".to_owned());
    }
    ReachabilityRecord::try_from(&fields)
}

pub(crate) fn parse_reachability_fields(
    fields: &csv::StringRecord,
) -> Result<ReachabilityRecord, String> {
    if fields.len() != 6 {
        return Err(format!("expected 6 CSV fields, found {}", fields.len()));
    }
    if fields[0].is_empty() || !fields[0].bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("input index must be a positive integer".to_owned());
    }
    let input_index = fields[0]
        .parse::<usize>()
        .map_err(|error| format!("invalid input index: {error}"))?;
    if input_index == 0 {
        return Err("input index must be at least 1".to_owned());
    }
    Ok(ReachabilityRecord {
        input_index,
        start: parse_position(&fields[1]).map_err(|error| format!("invalid start: {error}"))?,
        goal: parse_position(&fields[2]).map_err(|error| format!("invalid goal: {error}"))?,
        goal_match: fields[3].parse()?,
        transform: fields[4].parse()?,
        actions: parse_actions(&fields[5])?,
    })
}

/// 3分類 (OK/NG/UNKNOWN) を表す列挙型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriCategory {
    Ok,
    Ng,
    Unknown,
}

/// 結果を OK/NG/UNKNOWN に振り分けるトレイト
pub trait TriOutcome {
    fn category(&self) -> TriCategory;
}

/// 出力ディレクトリを作成し、TriOutputsを返す
pub fn ensure_tri_outputs(out_dir: &Path, prefix: &str) -> io::Result<TriOutputs> {
    fs::create_dir_all(out_dir)?;
    TriOutputs::new(out_dir, prefix)
}

/// 結果を3つのファイル（OK/NG/UNKNOWN）に書き出すための汎用構造体
pub struct TriOutputs {
    pub ok: io::BufWriter<File>,
    pub ng: io::BufWriter<File>,
    pub unknown: io::BufWriter<File>,
}

impl TriOutputs {
    fn new(out_dir: &Path, prefix: &str) -> io::Result<Self> {
        let ok = io::BufWriter::new(File::create_new(out_dir.join(format!("{prefix}_OK.txt")))?);
        let ng = io::BufWriter::new(File::create_new(out_dir.join(format!("{prefix}_NG.txt")))?);
        let unknown = io::BufWriter::new(File::create_new(
            out_dir.join(format!("{prefix}_UNKNOWN.txt")),
        )?);
        Ok(TriOutputs { ok, ng, unknown })
    }

    pub fn write_ok(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.ok, "{}", line)
    }

    pub fn write_ng(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.ng, "{}", line)
    }

    pub fn write_unknown(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.unknown, "{}", line)
    }

    pub fn write_result(&mut self, result: &impl TriOutcome, line: &str) -> io::Result<()> {
        match result.category() {
            TriCategory::Ok => self.write_ok(line),
            TriCategory::Ng => self.write_ng(line),
            TriCategory::Unknown => self.write_unknown(line),
        }
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.ok.flush()?;
        self.ng.flush()?;
        self.unknown.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const INITIAL: &str = "---------------------------OX------XO---------------------------";
    static FILE_INDEX: AtomicU64 = AtomicU64::new(0);

    fn parse_temporary_file<T>(
        contents: impl AsRef<[u8]>,
        parser: fn(&str) -> io::Result<T>,
    ) -> io::Result<T> {
        let path = std::env::temp_dir().join(format!(
            "othello-position-{}-{}.txt",
            std::process::id(),
            FILE_INDEX.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, contents).expect("write temporary input");
        let result = parser(path.to_str().expect("temporary path is UTF-8"));
        fs::remove_file(path).expect("remove temporary input");
        result
    }

    #[test]
    fn positions_round_trip_without_changing_relative_colors_or_turn() {
        for (suffix, color) in [('b', Color::Black), ('w', Color::White)] {
            let text = format!("{INITIAL} {suffix}");
            let position = parse_position(&text).unwrap();
            assert_eq!(position.board, Board::initial());
            assert_eq!(position.side_to_move, color);
            assert_eq!(format_position(position), text);
        }
    }

    #[test]
    fn position_input_requires_the_full_valid_position() {
        let invalid = [
            INITIAL.to_owned(),
            format!("{INITIAL} B"),
            format!("{INITIAL} q"),
            format!("{INITIAL} b extra"),
            format!("{INITIAL}  b"),
            format!("{INITIAL}\tb"),
            format!("{INITIAL} b "),
            format!("{} b", &INITIAL[..63]),
            format!("{INITIAL}- b"),
            format!("?{} b", &INITIAL[1..]),
            format!("{} b", "-".repeat(64)),
        ];
        for text in invalid {
            assert!(parse_position(&text).is_err(), "accepted {text:?}");
        }
    }

    #[test]
    fn position_files_preserve_order_duplicates_and_physical_error_lines() {
        let contents = format!("{INITIAL} w\n{INITIAL} b\n{INITIAL} w\n");
        let positions = parse_temporary_file(&contents, parse_file_to_positions).unwrap();
        assert_eq!(positions.len(), 3);
        assert_eq!(positions[0].side_to_move, Color::White);
        assert_eq!(positions[1].side_to_move, Color::Black);
        assert_eq!(positions[0], positions[2]);

        let invalid = format!("{INITIAL} b\n{INITIAL} w\n{INITIAL}\n");
        let error = parse_temporary_file(&invalid, parse_file_to_positions).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("line 3"));
        assert!(parse_temporary_file(" \n\n", parse_file_to_positions).is_err());
        assert!(parse_temporary_file("", parse_file_to_positions)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn unreadable_text_reports_the_physical_line() {
        let contents = [format!("{INITIAL} b\n").as_bytes(), &[0xff, b'\n']].concat();
        let error = parse_temporary_file(contents, parse_file_to_positions).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("line 2"));
    }

    #[test]
    fn action_tokens_preserve_empty_records_and_every_explicit_pass() {
        for text in ["", "ps", "ps A1", "A1 ps H8", "H8 ps"] {
            let actions = parse_actions(text).unwrap();
            assert_eq!(format_actions(&actions), text);
        }
        assert_eq!(
            parse_actions("A1 H8").unwrap(),
            vec![PlyAction::Play(0), PlyAction::Play(63)]
        );
        for text in [
            " ", " A1", "A1 ", "A1  ps", "A1\tps", "a1", "A0", "I1", "pass",
        ] {
            assert!(parse_actions(text).is_err(), "accepted {text:?}");
        }
    }

    #[test]
    fn symmetry_and_matching_identifiers_round_trip_strictly() {
        for symmetry in BoardSymmetry::ALL {
            assert_eq!(
                symmetry.as_str().parse::<BoardSymmetry>().unwrap(),
                symmetry
            );
        }
        for condition in [
            GoalMatch::Exact,
            GoalMatch::SymmetryOrbit,
            GoalMatch::EquivalenceClass,
        ] {
            assert_eq!(condition.as_str().parse::<GoalMatch>().unwrap(), condition);
        }
        assert!("unknown".parse::<BoardSymmetry>().is_err());
        assert!("Identity".parse::<BoardSymmetry>().is_err());
        assert!("unknown".parse::<GoalMatch>().is_err());
    }

    #[test]
    fn records_keep_six_columns_including_empty_actions() {
        let record = ReachabilityRecord {
            input_index: 2,
            start: Position::initial(),
            goal: Position::initial(),
            goal_match: GoalMatch::Exact,
            transform: crate::record::PositionTransform::IDENTITY,
            actions: Vec::new(),
        };
        let text = format!("2,{INITIAL} b,{INITIAL} b,exact,identity,");
        assert_eq!(record.to_csv(), text);
        for valid in [
            text.clone(),
            format!("{text}\r\n"),
            format!("\"2\",\"{INITIAL} b\",\"{INITIAL} b\",\"exact\",\"identity\",\"\""),
        ] {
            assert_eq!(ReachabilityRecord::from_csv(&valid).unwrap(), record);
        }
        for invalid in [
            String::new(),
            text.trim_end_matches(',').to_owned(),
            format!("{text},"),
            format!("{text}\n{text}"),
            text.replacen("2,", "0,", 1),
            text.replacen("2,", "-1,", 1),
            text.replace("identity", "unknown"),
            text.replace("exact", "unknown"),
            text.replace("identity", "\"identity,extra\""),
            text.replace("identity", "\"iden\"\"tity\""),
        ] {
            assert!(
                ReachabilityRecord::from_csv(&invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }
}
