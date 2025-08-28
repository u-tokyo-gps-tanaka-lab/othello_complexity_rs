use std::collections::HashSet;
use std::env;
use std::fs::File;
use std::io::Error;
use std::io::Write;
use std::io::{self, BufRead, BufReader};

use othello_complexity_rs::lib::othello::Board;
use othello_complexity_rs::lib::reverse_search::{retrospective_search, search};
use othello_complexity_rs::lib::solve_kissat::is_sat_ok;

// const DISCMAX: i32 = 15;
const DISCMAX: i32 = 5;

fn process_line(
    index: usize,
    line: &String,
    searched: &HashSet<[u64; 2]>,
    leafnode: &HashSet<[u64; 2]>,
    retrospective_searched: &mut HashSet<[u64; 2]>,
    retroflips: &mut Vec<[u64; 10_000]>,
) -> Result<bool, Error> {
    let mut player: u64 = 0;
    let mut opponent: u64 = 0;
    for (i, c) in line.chars().enumerate() {
        if c == 'X' {
            player |= 1_u64 << i;
        } else if c == 'O' {
            opponent |= 1_u64 << i;
        }
    }
    let bb = Board::new(player, opponent);
    let line = bb.to_string();
    if !is_sat_ok(0, &line).unwrap() {
        return Ok(false);
    } else {
        return Ok(true);
    }
    let uni = bb.unique();
    if searched.contains(&uni) || leafnode.contains(&uni) {
        return Ok(true);
    }
    retrospective_searched.clear();
    retroflips.resize(bb.popcount() as usize + 1, [0u64; 10_000]);
    let mut ans = false;
    if retrospective_search(
        &bb,
        false,
        DISCMAX,
        leafnode,
        retrospective_searched,
        retroflips,
    ) {
        println!("OK: {}", bb.to_string());
        println!("len(searched)={}", retrospective_searched.len());
        ans = true;
    } else {
        println!("NG: {}", bb.to_string());
    }
    Ok(ans)
}

fn process_file(filename: String) -> io::Result<()> {
    let file = File::open(&filename)?;
    let reader = BufReader::new(file);
    let mut okfile = File::create("sat_OK.txt")?;
    let mut ngfile = File::create("sat_NG.txt")?;
    let mut searched: HashSet<[u64; 2]> = HashSet::new();
    let mut leafnode: HashSet<[u64; 2]> = HashSet::new();
    for i in 5..=DISCMAX {
        searched.clear();
        leafnode.clear();
        let discs = i;
        let board = Board::initial();
        search(&board, &mut searched, &mut leafnode, discs);
        println!(
            "discs = {}: internal nodes = {}, leaf nodes = {}",
            i,
            searched.len(),
            leafnode.len()
        );
    }
    let mut retrospective_searched: HashSet<[u64; 2]> = HashSet::new();
    let mut retroflips: Vec<[u64; 10_000]> = vec![];
    // 1行ずつ読み込んで処理する
    for (index, line_result) in reader.lines().enumerate() {
        let line = line_result?; // Result<String> なのでアンラップ
        match process_line(
            index,
            &line,
            &searched,
            &leafnode,
            &mut retrospective_searched,
            &mut retroflips,
        ) {
            Ok(res) => {
                if res {
                    println!("{}", line);
                    writeln!(okfile, "{}", line);
                } else {
                    writeln!(ngfile, "{}", line);
                }
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    for (i, args) in args.iter().enumerate() {
        if i > 0 {
            if let Err(e) = process_file(args.to_string()) {
                eprintln!("Error: {}", e);
            }
        }
        println!("argv[{}] : {}", i, args);
    }
}
