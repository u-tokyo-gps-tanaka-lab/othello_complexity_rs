use crate::lib::othello::DXYS;

use std::collections::HashMap;
use std::fs::File;
use std::io::{Error, ErrorKind, Write};

//use rustsat::instances::SatInstance;
//use rustsat::instances::fio::dimacs;
use rustsat::solvers::Solve;
//use rustsat_kissat::Kissat;
//use rustsat::types::{Lit, Var, Clause};
use rustsat::types::{Clause, Lit};
// use rustsat::instances::{BasicVarManager, CnfFormula};
use rustsat::instances::Cnf;

struct VarMaker {
    count: i32,
}
impl VarMaker {
    pub fn new() -> Self {
        VarMaker { count: 0 }
    }
    fn mk_var(&mut self) -> i32 {
        self.count += 1;
        self.count
    }
    fn count(&self) -> usize {
        self.count as usize
    }
}

#[inline]
fn xy2sq(x: i32, y: i32) -> usize {
    (y * 8 + x) as usize
}

fn solve_by_kissat(
    index: usize,
    vs: &Vec<Vec<i32>>,
    num_var: usize,
    comment: &HashMap<usize, String>,
) -> bool {
    let mut solver = rustsat_kissat::Kissat::default();
    let mut cnf = Cnf::new();
    for line in vs {
        let mut clause = Clause::new();
        for i in 0..line.len() {
            if line[i] > 0 {
                clause.add(Lit::positive(line[i] as u32));
            } else {
                clause.add(Lit::negative((-line[i]) as u32));
            }
        }
        cnf.add_clause(clause);
    }
    solver.add_cnf(cnf);
    let result = match solver.solve() {
        Ok(res) => res,
        Err(e) => return false,
        //rustsat::solvers::SolverResult::Sat => println!("SAT: 解あり"),
        //rustsat::solvers::SolverResult::Unsat => println!("UNSAT: 解なし"),
        //rustsat::solvers::SolverResult::Unknown => println!("UNKNOWN: 解けませんでした"),
    };
    result == rustsat::solvers::SolverResult::Sat
    //eprintln!("result={:?}", result);
    //Ok(())
}

fn output_cnf(
    index: usize,
    vs: &Vec<Vec<i32>>,
    num_var: usize,
    comment: &HashMap<usize, String>,
) -> Result<(), Error> {
    let filename = format!("{}.cnf", index);
    let mut file = File::create(&filename)?;
    for (i, line) in comment.iter() {
        writeln!(file, "c Var_{}, {}", i, line.clone());
    }
    writeln!(file, "p cnf {} {}", num_var, vs.len());
    for line in vs {
        write!(file, "c ");
        for i in 0..line.len() {
            if i > 0 {
                write!(file, " ");
            }
            if line[i] > 0 {
                let v = line[i] as usize;
                write!(file, "{}", comment.get(&v).unwrap().to_string());
            } else {
                let v = (-line[i]) as usize;
                write!(file, "-{}", comment.get(&v).unwrap().to_string());
            }
        }
        writeln!(file, "");
        for i in 0..line.len() {
            if i > 0 {
                write!(file, " ");
            }
            write!(file, "{}", line[i]);
        }
        writeln!(file, " 0");
    }
    writeln!(file, "");
    // Ok(())
    Err(Error::new(ErrorKind::Other, "one cnf file only"))
}

pub fn is_sat_ok(index: usize, line: &String) -> Result<bool, Error> {
    eprintln!("line={}", line);
    let cs: Vec<char> = line.chars().collect();
    if cs.len() != 64 {
        return Err(Error::new(
            ErrorKind::Other,
            "length is not 64 format error",
        ));
    }
    let mut sqi: Vec<usize> = vec![];
    let mut sqo: Vec<usize> = vec![];
    let mut sqall: Vec<usize> = vec![];
    let mut vm = VarMaker::new();
    let mut in_sqo: Vec<bool> = vec![false; 64];
    for y in 0..8 {
        for x in 0..8 {
            let sq = xy2sq(x, y);
            if cs[sq] != '-' {
                sqall.push(sq);
                if 3 <= x && x <= 4 && 3 <= y && y <= 4 {
                    sqi.push(sq);
                } else {
                    sqo.push(sq);
                    in_sqo[sq] = true;
                }
            }
        }
    }
    if sqi.len() != 4 {
        return Err(Error::new(ErrorKind::Other, "empty squares in center 2x2"));
    }
    let sq33 = xy2sq(3, 3);

    // First[sq][col] : sqに最初に置かれる石がcolかどうかを表す論理変数
    let mut First: Vec<Vec<i32>> = vec![vec![0; 2]; 64];

    // Flip[sq][col] : [(sq', col, d, len)], sqをcolにflipするflip全体
    let mut Flip: Vec<Vec<Vec<(usize, usize, usize, usize)>>> = vec![vec![vec![]; 2]; 64];

    // Set[sq][col] : [(sq', col, d, len)], flipに加えて First[sq][col] に対応する(sq, col, 0, 0) も含む
    let mut Set: Vec<Vec<Vec<(usize, usize, usize, usize)>>> = vec![vec![vec![]; 2]; 64];

    // Base[sq][col] : [(sq', col, d, len)], sqがcolであることを利用してcolにflipするflip
    let mut Base: Vec<Vec<Vec<(usize, usize, usize, usize)>>> = vec![vec![vec![]; 2]; 64];

    // F[(sq, col, d, len)] : flip (sq, col, d, len) から論理変数への変換
    let mut F: HashMap<(usize, usize, usize, usize), i32> = HashMap::new();

    let v_sq33 = vm.mk_var();
    let mut comment: HashMap<usize, String> = HashMap::new();
    comment.insert(vm.count(), format!("Square33").to_string());
    for &sq in &sqall {
        let v = if in_sqo[sq] {
            comment.insert(vm.count() + 1, format!("Square_{}", sq).to_string());
            vm.mk_var()
        } else {
            v_sq33 * if sq / 8 == sq % 8 { 1 } else { -1 }
        };
        for col in 0..2 {
            let t = (sq, col, 0, 0);
            let v1 = if col == 0 { v } else { -v };
            First[sq][col] = v1;
            F.insert(t, v1);
            Set[sq][col].push(t);
        }
    }
    let mut Cmp: Vec<Vec<i32>> = vec![vec![0; 64]; 64];
    let mut s: Vec<Vec<i32>> = vec![];
    // eprintln!("sqo.len() = {}", sqo.len());
    for &sq in &sqo {
        for &sq1 in &sqo {
            if sq != sq1 {
                Cmp[sq][sq1] = vm.mk_var();
                comment.insert(vm.count(), format!("Cmp[{}][{}]", sq, sq1).to_string());
            }
        }
    }
    for &sq in &sqo {
        for &sq1 in &sqo {
            if sq != sq1 {
                if sq < sq1 {
                    // sq < sq1 かつ sq1 < sq となることはない．
                    s.push(vec![-Cmp[sq][sq1], -Cmp[sq1][sq]]);
                }
                for &sq2 in &sqo {
                    if sq2 != sq && sq2 != sq1 {
                        // 順序関係には推移律が成り立つ
                        s.push(vec![-Cmp[sq][sq2], -Cmp[sq2][sq1], Cmp[sq][sq1]]);
                    }
                }
            }
        }
    }
    //eprintln!("end of Cmp, s.len()={}", s.len());
    for &sq in &sqo {
        let x = (sq % 8) as i32;
        let y = (sq / 8) as i32;
        for col in 0..2 {
            let mut ps: Vec<i32> = vec![]; // sqにcolの石を置くすべてのflip
            for (d, (dx, dy)) in DXYS.iter().enumerate() {
                let mut sqs: Vec<usize> = vec![];
                let mut rl = 1;
                let mut x1 = x + dx;
                let mut y1 = y + dy;
                let mut samedir: Vec<i32> = vec![];
                while 0 <= x1 && x1 < 8 && 0 <= y1 && y1 < 8 && cs[xy2sq(x1, y1)] != '-' {
                    rl += 1;
                    let sq1 = xy2sq(x1, y1);
                    if rl >= 3 {
                        let t = (sq, col, d, rl);
                        let v = vm.mk_var();
                        comment.insert(vm.count(), format!("{:?}", t).to_string());
                        F.insert(t, v);
                        ps.push(v);
                        samedir.push(v);
                        for &sq2 in &sqs {
                            Flip[sq2][col].push(t);
                            Set[sq2][col].push(t);
                            if in_sqo[sq2] {
                                s.push(vec![-v, Cmp[sq2][sq]]);
                            }
                        }
                        Base[sq1][col].push(t);
                        if in_sqo[sq1] {
                            s.push(vec![-v, Cmp[sq1][sq]]);
                        }
                    }
                    sqs.push(sq1);
                    x1 += dx;
                    y1 += dy;
                }
                for i in 1..samedir.len() {
                    for j in 0..i {
                        s.push(vec![-samedir[i], -samedir[j]]);
                    }
                }
            }
            let mut line = vec![-First[sq][col]];
            for &f in &ps {
                // First[sq][1 - col] なら，psの中のflipはFalseになる．
                s.push(vec![-First[sq][1 - col], -f]);
                line.push(f);
            }
            // First[sq][col] なら，psの中のいずれかのflipがTrue
            s.push(line);
        }
    }
    //    eprintln!("end of First, s.len()={}", s.len());

    // Last
    // let mut Last: HashMap<(usize, (usize, usize, usize, usize)), i32> = HashMap::new();
    for &sq in &sqall {
        let last_c = if cs[sq] == 'X' { 1 } else { 0 };
        let mut vs = vec![];
        for &t in &Set[sq][last_c] {
            let v = *F.get(&t).unwrap();
            let v1 = vm.mk_var();
            comment.insert(vm.count(), format!("Last[{:?}]", t).to_string());
            vs.push(v1);
            s.push(vec![-v1, v]);
            for col in 0..2 {
                for &t1 in &Flip[sq][col] {
                    if t.0 != t1.0 && in_sqo[t.0] && in_sqo[t1.0] {
                        s.push(vec![-v1, -F.get(&t1).unwrap(), Cmp[t1.0][t.0]]);
                    }
                }
            }
        }
        for i in 1..vs.len() {
            for j in 0..i {
                s.push(vec![-vs[i], -vs[j]]);
            }
        }
        if vs.len() > 0 {
            s.push(vs);
        }
    }
    //eprintln!("end of Last, s.len()={}", s.len());
    // Before
    let mut Before: HashMap<
        (
            usize,
            (usize, usize, usize, usize),
            (usize, usize, usize, usize),
        ),
        i32,
    > = HashMap::new();
    for &sq in &sqo {
        for col in 0..2 {
            for &t in &Set[sq][col] {
                for &t1 in &Flip[sq][1 - col] {
                    if t.0 != t1.0 {
                        Before.insert((sq, t, t1), vm.mk_var());
                        comment.insert(
                            vm.count(),
                            format!("Before[({}, {:?}, {:?})]", sq, t, t1).to_string(),
                        );
                    }
                }
                for &t1 in &Base[sq][col] {
                    if t.0 != t1.0 {
                        Before.insert((sq, t, t1), vm.mk_var());
                        comment.insert(
                            vm.count(),
                            format!("Before[({}, {:?}, {:?})]", sq, t, t1).to_string(),
                        );
                    }
                }
            }
        }
    }
    for ((sq, t1, t2), v) in Before.iter() {
        if t1.3 != 0 || in_sqo[*sq] {
            s.push(vec![-v, Cmp[t1.0][t2.0]]);
        }
        s.push(vec![-v, *F.get(&t1).unwrap()]);
        s.push(vec![-v, *F.get(&t2).unwrap()]);
    }
    for &sq in &sqo {
        // let last_c = if cs[sq] == 'X' {1} else {0};
        for col in 0..2 {
            for &t1 in &Flip[sq][1 - col] {
                let mut vs: Vec<i32> = vec![-*F.get(&t1).unwrap()];
                for &t in &Set[sq][col] {
                    if t1.0 == t.0 {
                        continue;
                    }
                    vs.push(*Before.get(&(sq, t, t1)).unwrap());
                }
                s.push(vs);
            }
            for &t1 in &Base[sq][col] {
                let mut vs: Vec<i32> = vec![-*F.get(&t1).unwrap()];
                for &t in &Set[sq][col] {
                    if t1.0 == t.0 {
                        continue;
                    }
                    vs.push(*Before.get(&(sq, t, t1)).unwrap());
                }
                s.push(vs);
            }
        }
    }
    //output_cnf(index, &s, vm.count(), &comment)
    let ans = solve_by_kissat(index, &s, vm.count(), &comment);

    Ok(ans)
}
