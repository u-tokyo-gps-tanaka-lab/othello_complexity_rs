use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;

use othello_complexity_rs::lib::io::parse_file_to_boards_generic;
use othello_complexity_rs::lib::othello::{Geometry, Standard6x6, Standard8x8};
use othello_complexity_rs::lib::solve_kissat::{is_sat_ok, is_sat_ok6};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoardSize {
    Size8,
    Size6,
}

impl BoardSize {
    fn suffix(self) -> &'static str {
        match self {
            BoardSize::Size8 => "",
            BoardSize::Size6 => "_6x6",
        }
    }
}

fn parse_args() -> (BoardSize, Option<PathBuf>, Vec<String>) {
    let args: Vec<String> = env::args().collect();
    let mut out_dir: Option<PathBuf> = None;
    let mut inputs: Vec<String> = Vec::new();
    let mut board_size = BoardSize::Size8;

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-o" | "--out-dir" => {
                if i + 1 >= args.len() {
                    eprintln!("Option {} requires a directory path", arg);
                    process::exit(2);
                }
                out_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "-s" | "--size" => {
                if i + 1 >= args.len() {
                    eprintln!("--size には 6 または 8 を指定してください");
                    process::exit(2);
                }
                board_size = match args[i + 1].as_str() {
                    "6" | "6x6" => BoardSize::Size6,
                    "8" | "8x8" => BoardSize::Size8,
                    other => {
                        eprintln!("未知のサイズ指定: {} (6 または 8)", other);
                        process::exit(2);
                    }
                };
                i += 2;
            }
            "-h" | "--help" => {
                println!("Usage: check_sat [--size <6|8>] [-o DIR] <files...>");
                process::exit(0);
            }
            _ => {
                if let Some(rest) = arg.strip_prefix("--out-dir=") {
                    out_dir = Some(PathBuf::from(rest));
                    i += 1;
                    continue;
                }
                if let Some(rest) = arg.strip_prefix("-o=") {
                    out_dir = Some(PathBuf::from(rest));
                    i += 1;
                    continue;
                }
                if let Some(rest) = arg.strip_prefix("--size=") {
                    board_size = match rest {
                        "6" | "6x6" => BoardSize::Size6,
                        "8" | "8x8" => BoardSize::Size8,
                        other => {
                            eprintln!("未知のサイズ指定: {} (6 または 8)", other);
                            process::exit(2);
                        }
                    };
                    i += 1;
                    continue;
                }
                inputs.push(arg.clone());
                i += 1;
            }
        }
    }

    (board_size, out_dir, inputs)
}

fn process_file<G, F>(
    path: &str,
    out_dir: &Path,
    ok_name: &str,
    ng_name: &str,
    sat_check: F,
) -> io::Result<()>
where
    G: Geometry,
    F: Fn(usize, &str) -> Result<bool, std::io::Error>,
{
    let boards = parse_file_to_boards_generic::<G>(path)?;

    fs::create_dir_all(out_dir)?;
    let mut okfile = File::create(out_dir.join(ok_name))?;
    let mut ngfile = File::create(out_dir.join(ng_name))?;

    for (index, b) in boards.iter().enumerate() {
        let line = b.to_string();
        match sat_check(index, &line) {
            Ok(true) => {
                println!("{}", line);
                writeln!(okfile, "{}", line)?;
            }
            Ok(false) => {
                writeln!(ngfile, "{}", line)?;
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    }

    Ok(())
}

fn main() {
    let (board_size, out_dir_opt, inputs) = parse_args();
    let args: Vec<String> = env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        println!("argv[{}] : {}", i, arg);
    }

    if inputs.is_empty() {
        eprintln!("入力ファイルを指定してください");
        process::exit(1);
    }

    let out_dir_path =
        out_dir_opt.unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result"));

    let ok_filename = format!("sat_OK{}.txt", board_size.suffix());
    let ng_filename = format!("sat_NG{}.txt", board_size.suffix());

    for input in inputs {
        let res = match board_size {
            BoardSize::Size8 => process_file::<Standard8x8, _>(
                &input,
                &out_dir_path,
                &ok_filename,
                &ng_filename,
                |idx, line| is_sat_ok(idx, line),
            ),
            BoardSize::Size6 => process_file::<Standard6x6, _>(
                &input,
                &out_dir_path,
                &ok_filename,
                &ng_filename,
                |idx, line| is_sat_ok6(idx, line),
            ),
        };

        if let Err(e) = res {
            eprintln!("Error processing {}: {}", input, e);
        }
    }
}
