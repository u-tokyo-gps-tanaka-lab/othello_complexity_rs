use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};

use othello_complexity_rs::{
    io::{parse_file_to_boards, TriCategory, TriOutcome},
    othello::{board_with_symmetry, validate_board, Board},
    search::{
        core::SearchResult,
        strict::{retrospective_search_strict, StrictLeafCache},
        transposition::Btable,
    },
};

const ORBIT_DENOMINATOR: u64 = 16;

// Typical workflow:
//
// 1. Expand representative OK boards into concrete states that distinguish
//    board symmetries and the turn/color swap. Write outputs outside result/
//    when preserving existing experiment data.
//
//    cargo run --release --bin symmetry_distinguished -- expand \
//      result/thesis_layer_sat/layer_sat_OK.txt \
//      /tmp/layer_sat_OK_expanded.txt
//
// 2. Recheck the expanded concrete states without quotienting symmetries.
//
//    cargo run --release --bin symmetry_distinguished -- reverse-strict \
//      /tmp/layer_sat_OK_expanded.txt \
//      -o /tmp/reverse_strict_out \
//      --discs 15 \
//      --max-nodes 100000000
//
// 3. Summarize the representative-level results with orbit-size weights.
//
//    cargo run --release --bin symmetry_distinguished -- summarize \
//      --rep-ok result/thesis_layer_sat/layer_sat_OK.txt \
//      --rep-ng result/thesis_layer_sat/layer_sat_NG.txt \
//      --rep-unknown result/thesis_layer_sat/layer_sat_UNKNOWN.txt \
//      --strict-ok /tmp/reverse_strict_out/reverse_strict_OK.txt \
//      --strict-ng /tmp/reverse_strict_out/reverse_strict_NG.txt \
//      --strict-unknown /tmp/reverse_strict_out/reverse_strict_UNKNOWN.txt

#[derive(Parser, Debug)]
#[command(
    name = "symmetry_distinguished",
    about = "Experiments for counting Othello states without quotienting turn or board symmetries"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Expand canonical representative boards into their distinct 8-board-symmetry × turn-swap orbit.
    Expand(ExpandArgs),
    /// Run exact-orientation backward DFS and write reverse_strict_{OK,NG,UNKNOWN}.txt.
    #[command(name = "reverse-strict")]
    ReverseStrict(ReverseStrictArgs),
    /// Summarize representative-level results with orbit-size weights.
    Summarize(SummarizeArgs),
}

#[derive(Parser, Debug)]
struct ExpandArgs {
    /// Representative boards to expand.
    input: PathBuf,
    /// Output file for distinct expanded boards.
    output: PathBuf,
    /// Optional TSV file: representative, expanded board.
    #[arg(long)]
    map: Option<PathBuf>,
    /// Overwrite output files if they already exist.
    #[arg(long)]
    force: bool,
}

#[derive(Parser, Debug)]
struct ReverseStrictArgs {
    /// Input file containing concrete boards to check.
    input: PathBuf,
    /// Output directory for reverse_strict_{OK,NG,UNKNOWN}.txt.
    #[arg(short, long, value_name = "DIR")]
    out_dir: PathBuf,
    /// Number of discs at which to stop the forward search.
    #[arg(long, default_value_t = 15)]
    discs: i32,
    /// Maximum number of nodes to explore per input board.
    #[arg(long = "max-nodes", default_value_t = 1_000_000usize)]
    max_nodes: usize,
    /// Overwrite reverse_strict output files if they already exist.
    #[arg(long)]
    force: bool,
}

#[derive(Parser, Debug)]
struct SummarizeArgs {
    /// Representative boards already known reachable under the old quotient search.
    #[arg(long = "rep-ok")]
    rep_ok: PathBuf,
    /// Representative boards already known unreachable under the old quotient search.
    #[arg(long = "rep-ng")]
    rep_ng: PathBuf,
    /// Representative boards whose reachability is unknown under the old quotient search.
    #[arg(long = "rep-unknown")]
    rep_unknown: Option<PathBuf>,
    /// Strict OK file produced by reverse-strict for the expansion of rep-ok.
    #[arg(long = "strict-ok")]
    strict_ok: Option<PathBuf>,
    /// Strict NG file produced by reverse-strict for the expansion of rep-ok.
    #[arg(long = "strict-ng")]
    strict_ng: Option<PathBuf>,
    /// Strict UNKNOWN file produced by reverse-strict for the expansion of rep-ok.
    #[arg(long = "strict-unknown")]
    strict_unknown: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct UnitCounts {
    ok: u64,
    ng: u64,
    unknown: u64,
}

impl UnitCounts {
    fn total(&self) -> u64 {
        self.ok + self.ng + self.unknown
    }
}

struct StrictOutputs {
    ok: BufWriter<File>,
    ng: BufWriter<File>,
    unknown: BufWriter<File>,
}

impl StrictOutputs {
    fn new(out_dir: &Path, force: bool) -> io::Result<Self> {
        fs::create_dir_all(out_dir)?;
        Ok(Self {
            ok: BufWriter::new(open_output(&out_dir.join("reverse_strict_OK.txt"), force)?),
            ng: BufWriter::new(open_output(&out_dir.join("reverse_strict_NG.txt"), force)?),
            unknown: BufWriter::new(open_output(
                &out_dir.join("reverse_strict_UNKNOWN.txt"),
                force,
            )?),
        })
    }

    fn write_result(&mut self, result: &SearchResult, board: &Board) -> io::Result<()> {
        let line = board.to_string();
        match result.category() {
            TriCategory::Ok => writeln!(self.ok, "{line}"),
            TriCategory::Ng => writeln!(self.ng, "{line}"),
            TriCategory::Unknown => writeln!(self.unknown, "{line}"),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.ok.flush()?;
        self.ng.flush()?;
        self.unknown.flush()?;
        Ok(())
    }
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Expand(args) => run_expand(args),
        Command::ReverseStrict(args) => run_reverse_strict(args),
        Command::Summarize(args) => run_summarize(args),
    }
}

fn run_expand(args: ExpandArgs) -> io::Result<()> {
    let reps = read_boards(&args.input)?;
    let mut out = BufWriter::new(open_output(&args.output, args.force)?);
    let mut map = match args.map {
        Some(path) => Some(BufWriter::new(open_output(&path, args.force)?)),
        None => None,
    };

    let mut total_reps = 0_usize;
    let mut total_expanded = 0_usize;
    for rep in reps {
        total_reps += 1;
        let rep_line = rep.to_string();
        let variants = symmetry_turn_orbit(rep);
        total_expanded += variants.len();
        for variant in variants {
            let variant_line = variant.to_string();
            writeln!(out, "{variant_line}")?;
            if let Some(map) = map.as_mut() {
                writeln!(map, "{rep_line}\t{variant_line}")?;
            }
        }
    }
    out.flush()?;
    if let Some(map) = map.as_mut() {
        map.flush()?;
    }

    println!("representatives = {total_reps}");
    println!("expanded distinct states = {total_expanded}");
    Ok(())
}

fn run_reverse_strict(args: ReverseStrictArgs) -> io::Result<()> {
    let boards = read_boards(&args.input)?;
    let mut outputs = StrictOutputs::new(&args.out_dir, args.force)?;
    let leaf_cache = StrictLeafCache::new(args.discs);
    println!(
        "strict forward cache: discs = {}, internal = {}, leaf = {}",
        args.discs,
        leaf_cache.searched_count(),
        leaf_cache.leaf_count()
    );

    let mut visited = Btable::new(0x100000000, 0x10000);
    let mut retroflips = Vec::new();
    for board in boards {
        if validate_board(&board).is_err() {
            outputs.write_result(&SearchResult::NotFound, &board)?;
            continue;
        }

        visited.clear();
        let mut node_count = 0_usize;
        let result = retrospective_search_strict(
            &board,
            false,
            args.discs,
            leaf_cache.leaf(),
            &mut visited,
            &mut retroflips,
            &mut node_count,
            args.max_nodes,
        );
        outputs.write_result(&result, &board)?;
        outputs.flush()?;
    }
    outputs.flush()
}

fn run_summarize(args: SummarizeArgs) -> io::Result<()> {
    let rep_ok = read_boards(&args.rep_ok)?;
    let rep_ng = read_boards(&args.rep_ng)?;
    let rep_unknown = match args.rep_unknown.as_ref() {
        Some(path) => read_boards(path)?,
        None => Vec::new(),
    };

    let strict = read_strict_categories(&args)?;
    let mut counts = UnitCounts::default();

    for rep in rep_ng {
        counts.ng += orbit_units(rep);
    }
    for rep in rep_unknown {
        counts.unknown += orbit_units(rep);
    }
    for rep in rep_ok {
        let variants = symmetry_turn_orbit(rep);
        if let Some(strict) = strict.as_ref() {
            for variant in variants {
                match strict.get(&variant.to_string()).copied() {
                    Some(TriCategory::Ok) => counts.ok += 1,
                    Some(TriCategory::Ng) => counts.ng += 1,
                    Some(TriCategory::Unknown) => counts.unknown += 1,
                    None => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "missing strict classification for expanded board: {}",
                                variant.to_string()
                            ),
                        ));
                    }
                }
            }
        } else {
            counts.ok += variants.len() as u64;
        }
    }

    print_summary(&counts);
    Ok(())
}

fn read_boards(path: &Path) -> io::Result<Vec<Board>> {
    parse_file_to_boards(&path.to_string_lossy())
}

fn read_strict_categories(
    args: &SummarizeArgs,
) -> io::Result<Option<HashMap<String, TriCategory>>> {
    let any = args.strict_ok.is_some() || args.strict_ng.is_some() || args.strict_unknown.is_some();
    if !any {
        return Ok(None);
    }
    let mut categories = HashMap::new();
    if let Some(path) = args.strict_ok.as_ref() {
        insert_categories(&mut categories, path, TriCategory::Ok)?;
    }
    if let Some(path) = args.strict_ng.as_ref() {
        insert_categories(&mut categories, path, TriCategory::Ng)?;
    }
    if let Some(path) = args.strict_unknown.as_ref() {
        insert_categories(&mut categories, path, TriCategory::Unknown)?;
    }
    Ok(Some(categories))
}

fn insert_categories(
    categories: &mut HashMap<String, TriCategory>,
    path: &Path,
    category: TriCategory,
) -> io::Result<()> {
    for board in read_boards_allow_empty(path)? {
        let key = board.to_string();
        if let Some(prev) = categories.insert(key.clone(), category) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "board appears in multiple strict result files ({prev:?}, {category:?}): {key}"
                ),
            ));
        }
    }
    Ok(())
}

fn read_boards_allow_empty(path: &Path) -> io::Result<Vec<Board>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut boards = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let filtered: String = line
            .chars()
            .filter(|&c| c == 'X' || c == 'O' || c == '-')
            .collect();
        if filtered.len() == 64 {
            if let Some(board) = othello_complexity_rs::io::parse_line_to_board(&filtered) {
                boards.push(board);
            }
        }
    }
    Ok(boards)
}

fn symmetry_turn_orbit(board: Board) -> Vec<Board> {
    let mut set = BTreeSet::new();
    for turn_swap in [false, true] {
        let base = if turn_swap { board.swapped() } else { board };
        for sym in 0..8_i32 {
            set.insert(board_with_symmetry(base, sym));
        }
    }
    set.into_iter().collect()
}

fn orbit_units(board: Board) -> u64 {
    symmetry_turn_orbit(board).len() as u64
}

fn open_output(path: &Path, force: bool) -> io::Result<File> {
    if force {
        File::create(path)
    } else {
        OpenOptions::new().write(true).create_new(true).open(path)
    }
}

fn print_summary(counts: &UnitCounts) {
    let total = counts.total();
    println!("orbit denominator = {ORBIT_DENOMINATOR}");
    println!("ok units = {}", counts.ok);
    println!("ng units = {}", counts.ng);
    println!("unknown units = {}", counts.unknown);
    println!("total units = {total}");
    println!(
        "weighted ok = {:.6}",
        counts.ok as f64 / ORBIT_DENOMINATOR as f64
    );
    println!(
        "weighted ng = {:.6}",
        counts.ng as f64 / ORBIT_DENOMINATOR as f64
    );
    println!(
        "weighted unknown = {:.6}",
        counts.unknown as f64 / ORBIT_DENOMINATOR as f64
    );
    println!(
        "weighted total = {:.6}",
        total as f64 / ORBIT_DENOMINATOR as f64
    );
    if total > 0 {
        println!(
            "lower reachable proportion = {:.12}",
            counts.ok as f64 / total as f64
        );
        println!(
            "upper reachable proportion = {:.12}",
            (counts.ok + counts.unknown) as f64 / total as f64
        );
    }
}
