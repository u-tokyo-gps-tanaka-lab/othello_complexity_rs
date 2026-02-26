use crate::othello::{Board, Direction};

use std::collections::HashMap;
use std::fs::File;
use std::io::{Error, ErrorKind, Write};

use rustsat::{
    instances::Cnf,
    solvers::Solve,
    types::{Clause, Lit},
};

/// flip操作を識別する構造体
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlipId {
    pub sq: usize,  // 石を配置するマス (0-63)
    pub col: usize, // 色 (0=黒, 1=白)
    pub dir: usize, // 方向 (0-7, Direction enumに対応)
    pub len: usize, // 長さ (0=First配置, 3以上=flip操作)
}

impl FlipId {
    /// 最初の配置（flipではない）
    pub fn first(sq: usize, col: usize) -> Self {
        FlipId {
            sq,
            col,
            dir: 0,
            len: 0,
        }
    }

    /// flip操作
    pub fn flip(sq: usize, col: usize, dir: usize, len: usize) -> Self {
        debug_assert!(len >= 3, "flip length must be >= 3");
        FlipId { sq, col, dir, len }
    }

    /// 最初の配置かどうか
    pub fn is_first(&self) -> bool {
        self.len == 0
    }

    /// タプルへの変換
    pub fn to_tuple(&self) -> (usize, usize, usize, usize) {
        (self.sq, self.col, self.dir, self.len)
    }

    /// タプルからの変換
    pub fn from_tuple(t: (usize, usize, usize, usize)) -> Self {
        FlipId {
            sq: t.0,
            col: t.1,
            dir: t.2,
            len: t.3,
        }
    }
}

/// SAT問題の命題変数の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SatVar {
    /// 中央マスの共有変数 (v_sq33)
    /// first[center_sq][col] は CenterBase から符号調整で導出
    CenterBase,

    /// 外側マスの変数 (Square_sq)
    /// first[sq][col] は OuterSquare から符号で導出
    OuterSquare { sq: usize },

    /// flip操作が行われた
    Flip(FlipId),

    /// 順序関係: sq1がsq2より先に石が置かれた (cmp[sq1][sq2])
    Cmp { sq1: usize, sq2: usize },

    /// flip_idがマスsqの最終状態を決定した
    Last { sq: usize, flip_id: FlipId },

    /// マスsqにおいて、t1がt2より先に影響した
    Before { sq: usize, t1: FlipId, t2: FlipId },
}

/// SAT制約を構築するビルダー
pub struct ClauseBuilder {
    clauses: Vec<Vec<i32>>,
    registry: HashMap<SatVar, i32>,
    var_count: i32,
    comments: HashMap<usize, String>,
}

impl ClauseBuilder {
    pub fn new() -> Self {
        ClauseBuilder {
            clauses: vec![],
            registry: HashMap::new(),
            var_count: 0,
            comments: HashMap::new(),
        }
    }

    // ===== 変数管理 =====

    /// 変数を取得または新規作成
    pub fn get_or_create(&mut self, var: SatVar) -> i32 {
        if let Some(&v) = self.registry.get(&var) {
            return v;
        }
        self.var_count += 1;
        let v = self.var_count;
        self.registry.insert(var, v);
        self.comments.insert(v as usize, format!("{:?}", var));
        v
    }

    /// first[sq][col] に対応するリテラルを取得
    /// - 外側マス: OuterSquare { sq } の正/負のリテラル
    /// - 中央マス: CenterBase から符号調整で導出
    pub fn get_first_literal(&mut self, sq: usize, col: usize, board: &BoardInfo) -> i32 {
        if board.in_outer[sq] {
            // 外側マス: 専用変数
            let base = self.get_or_create(SatVar::OuterSquare { sq });
            if col == 0 {
                base
            } else {
                -base
            }
        } else {
            // 中央マス: 共有変数 + 符号調整
            // 対角線上(sq/8 == sq%8)なら正、そうでなければ負
            let base = self.get_or_create(SatVar::CenterBase);
            let is_diagonal = sq / 8 == sq % 8;
            let sign = if is_diagonal { 1 } else { -1 };
            let col_sign = if col == 0 { 1 } else { -1 };
            base * sign * col_sign
        }
    }

    // ===== 意味のある制約ヘルパー =====

    /// 高々1つが真 (At-Most-One)
    /// ∀i≠j: ¬xi ∨ ¬xj
    pub fn at_most_one(&mut self, vars: &[i32]) {
        for i in 1..vars.len() {
            for j in 0..i {
                self.clauses.push(vec![-vars[i], -vars[j]]);
            }
        }
    }

    /// 少なくとも1つが真 (At-Least-One)
    /// x1 ∨ x2 ∨ ... ∨ xn
    pub fn at_least_one(&mut self, vars: &[i32]) {
        self.clauses.push(vars.to_vec());
    }

    /// 丁度1つが真 (Exactly-One)
    pub fn exactly_one(&mut self, vars: &[i32]) {
        self.at_least_one(vars);
        self.at_most_one(vars);
    }

    /// 含意: a → b
    /// ¬a ∨ b
    pub fn implies(&mut self, a: i32, b: i32) {
        self.clauses.push(vec![-a, b]);
    }

    /// 複合含意: (a1 ∧ a2 ∧ ...) → b
    /// ¬a1 ∨ ¬a2 ∨ ... ∨ b
    pub fn implies_all(&mut self, antecedents: &[i32], consequent: i32) {
        let mut clause: Vec<i32> = antecedents.iter().map(|&a| -a).collect();
        clause.push(consequent);
        self.clauses.push(clause);
    }

    /// 反対称性: ¬(a<b ∧ b<a)
    pub fn add_antisymmetry(&mut self, ab: i32, ba: i32) {
        self.clauses.push(vec![-ab, -ba]);
    }

    /// 推移律: (a<b ∧ b<c) → a<c
    pub fn add_transitivity(&mut self, ab: i32, bc: i32, ac: i32) {
        self.clauses.push(vec![-ab, -bc, ac]);
    }

    /// 生の節を追加（移行用）
    pub fn push(&mut self, clause: Vec<i32>) {
        self.clauses.push(clause);
    }

    // ===== アクセサ =====

    pub fn clauses(&self) -> &Vec<Vec<i32>> {
        &self.clauses
    }

    pub fn var_count(&self) -> usize {
        self.var_count as usize
    }

    pub fn comments(&self) -> &HashMap<usize, String> {
        &self.comments
    }
}

/// 盤面の解析情報
pub struct BoardInfo {
    pub player: u64,
    pub opponent: u64,
    pub occupied: u64,
    pub center_squares: Vec<usize>, // 中央4マス [27, 28, 35, 36]
    pub outer_squares: Vec<usize>,  // 中央以外の占有マス
    pub all_squares: Vec<usize>,    // すべての占有マス
    pub in_outer: [bool; 64],       // outer_squaresに含まれるかの高速判定
}

impl BoardInfo {
    /// 盤面を検証し、解析情報を構築
    pub fn new(player: u64, opponent: u64) -> Result<Self, Error> {
        if player & opponent != 0 {
            return Err(Error::new(ErrorKind::Other, "player and opponent overlap"));
        }

        let occupied = player | opponent;
        let mut center_squares = vec![];
        let mut outer_squares = vec![];
        let mut all_squares = vec![];
        let mut in_outer = [false; 64];

        for y in 0..8 {
            for x in 0..8 {
                let sq = y * 8 + x;
                if occupied & (1u64 << sq) != 0 {
                    all_squares.push(sq);
                    if 3 <= x && x <= 4 && 3 <= y && y <= 4 {
                        center_squares.push(sq);
                    } else {
                        outer_squares.push(sq);
                        in_outer[sq] = true;
                    }
                }
            }
        }

        if center_squares.len() != 4 {
            return Err(Error::new(ErrorKind::Other, "empty squares in center 2x2"));
        }

        Ok(BoardInfo {
            player,
            opponent,
            occupied,
            center_squares,
            outer_squares,
            all_squares,
            in_outer,
        })
    }

    /// マスsqの最終的な色を取得 (0=player(X), 1=opponent(O))
    pub fn final_color(&self, sq: usize) -> usize {
        if (self.player & (1u64 << sq)) != 0 {
            0
        } else {
            1
        }
    }

    /// マスsqがプレイヤーの石かどうか
    pub fn is_player(&self, sq: usize) -> bool {
        (self.player & (1u64 << sq)) != 0
    }
}

/// 各マスに対するflip操作の情報
/// 変数マッピングはClauseBuilderが管理するため、データ構造のみ保持
pub struct FlipInfo {
    /// flip[sq][col]: マスsqをcol色にflipする操作のリスト
    pub flip: Vec<Vec<Vec<FlipId>>>,

    /// set[sq][col]: flip[sq][col] + First配置
    pub set: Vec<Vec<Vec<FlipId>>>,

    /// base[sq][col]: マスsqがcol色であることを端点として利用するflip操作
    pub base: Vec<Vec<Vec<FlipId>>>,
}

impl FlipInfo {
    /// 盤面情報から変数とデータ構造を構築（制約は追加しない）
    /// 変数はClauseBuilderのget_or_create/get_first_literalで管理
    pub fn compute(board: &BoardInfo, builder: &mut ClauseBuilder) -> Self {
        let mut flip: Vec<Vec<Vec<FlipId>>> = vec![vec![vec![]; 2]; 64];
        let mut set: Vec<Vec<Vec<FlipId>>> = vec![vec![vec![]; 2]; 64];
        let mut base: Vec<Vec<Vec<FlipId>>> = vec![vec![vec![]; 2]; 64];

        // CenterBase / OuterSquare 変数の作成 + set への First 登録
        for &sq in &board.all_squares {
            // 変数を作成（get_first_literal が内部で get_or_create を呼ぶ）
            builder.get_first_literal(sq, 0, board);
            // set にFirstを登録
            for col in 0..2 {
                let flip_id = FlipId::first(sq, col);
                set[sq][col].push(flip_id);
            }
        }

        // Cmp変数の作成
        for &sq in &board.outer_squares {
            for &sq1 in &board.outer_squares {
                if sq != sq1 {
                    builder.get_or_create(SatVar::Cmp { sq1: sq, sq2: sq1 });
                }
            }
        }

        // Flip変数の作成とflip/set/baseデータ構造の構築
        for &sq in &board.outer_squares {
            let x = (sq % 8) as i32;
            let y = (sq / 8) as i32;
            for col in 0..2 {
                for (d, direction) in Direction::all().iter().enumerate() {
                    let (dx, dy) = direction.to_offset();
                    let mut sqs: Vec<usize> = vec![];
                    let mut rl = 1;
                    let mut x1 = x + dx;
                    let mut y1 = y + dy;

                    while 0 <= x1
                        && x1 < 8
                        && 0 <= y1
                        && y1 < 8
                        && (board.occupied & (1u64 << (y1 * 8 + x1))) != 0
                    {
                        rl += 1;
                        let sq1 = (y1 * 8 + x1) as usize;
                        if rl >= 3 {
                            let flip_id = FlipId::flip(sq, col, d, rl);
                            builder.get_or_create(SatVar::Flip(flip_id));

                            // flip/set/baseデータ構造の構築
                            for &sq2 in &sqs {
                                flip[sq2][col].push(flip_id);
                                set[sq2][col].push(flip_id);
                            }
                            base[sq1][col].push(flip_id);
                        }
                        sqs.push(sq1);
                        x1 += dx;
                        y1 += dy;
                    }
                }
            }
        }

        FlipInfo { flip, set, base }
    }
}

/// 順序変数の制約を追加
/// cmp変数の反対称性と推移律
fn add_ordering_constraints(builder: &mut ClauseBuilder, board: &BoardInfo, _flip_info: &FlipInfo) {
    for &sq in &board.outer_squares {
        for &sq1 in &board.outer_squares {
            if sq != sq1 {
                let cmp_sq_sq1 = builder.get_or_create(SatVar::Cmp { sq1: sq, sq2: sq1 });
                let cmp_sq1_sq = builder.get_or_create(SatVar::Cmp { sq1: sq1, sq2: sq });

                // 反対称性: sq < sq1 かつ sq1 < sq となることはない
                if sq < sq1 {
                    builder.add_antisymmetry(cmp_sq_sq1, cmp_sq1_sq);
                }
                // 推移律: (sq < sq2 ∧ sq2 < sq1) → sq < sq1
                for &sq2 in &board.outer_squares {
                    if sq2 != sq && sq2 != sq1 {
                        let cmp_sq_sq2 = builder.get_or_create(SatVar::Cmp { sq1: sq, sq2: sq2 });
                        let cmp_sq2_sq1 = builder.get_or_create(SatVar::Cmp { sq1: sq2, sq2: sq1 });
                        builder.add_transitivity(cmp_sq_sq2, cmp_sq2_sq1, cmp_sq_sq1);
                    }
                }
            }
        }
    }
}

/// flip操作の制約を追加
/// - flip変数が真なら、経路上のマスはflipより先に置かれた
/// - 同じ方向のflipは高々1つ
/// - First[sq][1-col] → flipはFalse
/// - First[sq][col] → いずれかのflipがTrue
fn add_flip_constraints(builder: &mut ClauseBuilder, board: &BoardInfo, _flip_info: &FlipInfo) {
    for &sq in &board.outer_squares {
        let x = (sq % 8) as i32;
        let y = (sq / 8) as i32;
        for col in 0..2 {
            let mut ps: Vec<i32> = vec![]; // sqにcolの石を置くすべてのflip変数
            for (d, direction) in Direction::all().iter().enumerate() {
                let (dx, dy) = direction.to_offset();
                let mut sqs: Vec<usize> = vec![]; // 経路上のマス
                let mut rl = 1;
                let mut x1 = x + dx;
                let mut y1 = y + dy;
                let mut samedir: Vec<i32> = vec![]; // 同じ方向のflip変数

                while 0 <= x1
                    && x1 < 8
                    && 0 <= y1
                    && y1 < 8
                    && (board.occupied & (1u64 << (y1 * 8 + x1))) != 0
                {
                    rl += 1;
                    let sq1 = (y1 * 8 + x1) as usize;
                    if rl >= 3 {
                        let flip_id = FlipId::flip(sq, col, d, rl);
                        let v = builder.get_or_create(SatVar::Flip(flip_id));
                        ps.push(v);
                        samedir.push(v);

                        // flip変数が真なら、経路上のマスはflipより先に置かれた
                        for &sq2 in &sqs {
                            if board.in_outer[sq2] {
                                let cmp = builder.get_or_create(SatVar::Cmp { sq1: sq2, sq2: sq });
                                builder.implies(v, cmp);
                            }
                        }
                        // 端点のマスも同様
                        if board.in_outer[sq1] {
                            let cmp = builder.get_or_create(SatVar::Cmp { sq1: sq1, sq2: sq });
                            builder.implies(v, cmp);
                        }
                    }
                    sqs.push(sq1);
                    x1 += dx;
                    y1 += dy;
                }

                // 同じ方向のflipは高々1つ
                builder.at_most_one(&samedir);
            }

            // First[sq][1-col] → flipはFalse
            let first_other = builder.get_first_literal(sq, 1 - col, board);
            for &flip_v in &ps {
                builder.implies(first_other, -flip_v);
            }

            // First[sq][col] → いずれかのflipがTrue
            // -First[sq][col] ∨ flip1 ∨ flip2 ∨ ...
            let first_col = builder.get_first_literal(sq, col, board);
            let mut line = vec![-first_col];
            line.extend(ps);
            builder.push(line);
        }
    }
}

/// Last制約を追加
/// 各マスの最終状態を決定するflip操作について、丁度1つが選ばれる制約
fn add_last_constraints(builder: &mut ClauseBuilder, board: &BoardInfo, flip_info: &FlipInfo) {
    for &sq in &board.all_squares {
        // 最終的な色（playerなら0, opponentなら1 → 逆転してlast_cを決定）
        let last_c = if board.is_player(sq) { 1 } else { 0 };

        let mut vs = vec![];
        for flip_id in &flip_info.set[sq][last_c] {
            // FlipIdに対応する変数を取得
            let v = if flip_id.is_first() {
                builder.get_first_literal(flip_id.sq, flip_id.col, board)
            } else {
                builder.get_or_create(SatVar::Flip(*flip_id))
            };

            let v1 = builder.get_or_create(SatVar::Last {
                sq,
                flip_id: *flip_id,
            });
            vs.push(v1);

            // v1 -> v (Last[t] ならば t が真)
            builder.implies(v1, v);

            // 他のflipとの順序関係
            for col in 0..2 {
                for other_flip in &flip_info.flip[sq][col] {
                    if flip_id.sq != other_flip.sq
                        && board.in_outer[flip_id.sq]
                        && board.in_outer[other_flip.sq]
                    {
                        let other_v = builder.get_or_create(SatVar::Flip(*other_flip));
                        let cmp = builder.get_or_create(SatVar::Cmp {
                            sq1: other_flip.sq,
                            sq2: flip_id.sq,
                        });
                        // v1 && other_v -> cmp[other.sq][t.sq]
                        builder.implies_all(&[v1, other_v], cmp);
                    }
                }
            }
        }

        // 高々1つのLastが選ばれる
        builder.at_most_one(&vs);

        // 少なくとも1つのLastが選ばれる
        if !vs.is_empty() {
            builder.at_least_one(&vs);
        }
    }
}

/// FlipIdに対応するSAT変数/リテラルを取得するヘルパー
fn get_flip_var(builder: &mut ClauseBuilder, flip_id: &FlipId, board: &BoardInfo) -> i32 {
    if flip_id.is_first() {
        builder.get_first_literal(flip_id.sq, flip_id.col, board)
    } else {
        builder.get_or_create(SatVar::Flip(*flip_id))
    }
}

/// Before制約を追加
/// マスsqにおいて、t1がt2より先に影響したことを表す制約
fn add_before_constraints(builder: &mut ClauseBuilder, board: &BoardInfo, flip_info: &FlipInfo) {
    // Before変数の作成（get_or_createで自動管理）
    // まず変数を作成
    for &sq in &board.outer_squares {
        for col in 0..2 {
            for &t in &flip_info.set[sq][col] {
                for &t1 in &flip_info.flip[sq][1 - col] {
                    if t.sq != t1.sq {
                        builder.get_or_create(SatVar::Before { sq, t1: t, t2: t1 });
                    }
                }
                for &t1 in &flip_info.base[sq][col] {
                    if t.sq != t1.sq {
                        builder.get_or_create(SatVar::Before { sq, t1: t, t2: t1 });
                    }
                }
            }
        }
    }

    // Before変数の制約
    for &sq in &board.outer_squares {
        for col in 0..2 {
            for &t in &flip_info.set[sq][col] {
                for &t1 in &flip_info.flip[sq][1 - col] {
                    if t.sq != t1.sq {
                        let v = builder.get_or_create(SatVar::Before { sq, t1: t, t2: t1 });
                        // t.len != 0 または sqがouterなら順序制約を追加
                        if !t.is_first() || board.in_outer[sq] {
                            let cmp = builder.get_or_create(SatVar::Cmp {
                                sq1: t.sq,
                                sq2: t1.sq,
                            });
                            builder.implies(v, cmp);
                        }
                        // Before[t,t1] -> t && t1
                        let t_var = get_flip_var(builder, &t, board);
                        let t1_var = get_flip_var(builder, &t1, board);
                        builder.implies(v, t_var);
                        builder.implies(v, t1_var);
                    }
                }
                for &t1 in &flip_info.base[sq][col] {
                    if t.sq != t1.sq {
                        let v = builder.get_or_create(SatVar::Before { sq, t1: t, t2: t1 });
                        if !t.is_first() || board.in_outer[sq] {
                            let cmp = builder.get_or_create(SatVar::Cmp {
                                sq1: t.sq,
                                sq2: t1.sq,
                            });
                            builder.implies(v, cmp);
                        }
                        let t_var = get_flip_var(builder, &t, board);
                        let t1_var = get_flip_var(builder, &t1, board);
                        builder.implies(v, t_var);
                        builder.implies(v, t1_var);
                    }
                }
            }
        }
    }

    // flip/baseに対する制約
    for &sq in &board.outer_squares {
        for col in 0..2 {
            // flip[sq][1-col]に対する制約
            for &t1 in &flip_info.flip[sq][1 - col] {
                let t1_var = get_flip_var(builder, &t1, board);
                let mut vs: Vec<i32> = vec![-t1_var];
                for &t in &flip_info.set[sq][col] {
                    if t1.sq == t.sq {
                        continue;
                    }
                    let before_v = builder.get_or_create(SatVar::Before { sq, t1: t, t2: t1 });
                    vs.push(before_v);
                }
                builder.push(vs);
            }
            // base[sq][col]に対する制約
            for &t1 in &flip_info.base[sq][col] {
                let t1_var = get_flip_var(builder, &t1, board);
                let mut vs: Vec<i32> = vec![-t1_var];
                for &t in &flip_info.set[sq][col] {
                    if t1.sq == t.sq {
                        continue;
                    }
                    let before_v = builder.get_or_create(SatVar::Before { sq, t1: t, t2: t1 });
                    vs.push(before_v);
                }
                builder.push(vs);
            }
        }
    }
}

fn solve_by_kissat(vs: &Vec<Vec<i32>>, _num_var: usize, _comment: &HashMap<usize, String>) -> bool {
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
    if let Err(_) = solver.add_cnf(cnf) {
        return false;
    }
    let result = match solver.solve() {
        Ok(res) => res,
        Err(_) => return false,
    };
    result == rustsat::solvers::SolverResult::Sat
}

#[allow(dead_code)]
fn output_cnf(
    filename: usize,
    vs: &Vec<Vec<i32>>,
    num_var: usize,
    comment: &HashMap<usize, String>,
) -> Result<(), Error> {
    let filename = format!("{}.cnf", filename);
    let mut file = File::create(&filename)?;
    for (i, line) in comment.iter() {
        writeln!(file, "c Var_{}, {}", i, line)?;
    }
    writeln!(file, "p cnf {} {}", num_var, vs.len())?;
    for line in vs {
        write!(file, "c ")?;
        for i in 0..line.len() {
            if i > 0 {
                write!(file, " ")?;
            }
            if line[i] > 0 {
                let v = line[i] as usize;
                write!(file, "{}", comment.get(&v).unwrap())?;
            } else {
                let v = (-line[i]) as usize;
                write!(file, "-{}", comment.get(&v).unwrap())?;
            }
        }
        writeln!(file, "")?;
        for i in 0..line.len() {
            if i > 0 {
                write!(file, " ")?;
            }
            write!(file, "{}", line[i])?;
        }
        writeln!(file, " 0")?;
    }
    writeln!(file, "")?;
    Err(Error::new(ErrorKind::Other, "one cnf file only"))
}

pub fn is_sat_ok(player: u64, opponent: u64, verbose: bool) -> Result<bool, Error> {
    // 1. 盤面検証・解析
    let board = BoardInfo::new(player, opponent)?;

    // 2. 制約ビルダー初期化
    let mut builder = ClauseBuilder::new();

    // 3. 変数とデータ構造の作成（制約なし）
    let flip_info = FlipInfo::compute(&board, &mut builder);

    // 4. 制約の宣言的な追加
    add_ordering_constraints(&mut builder, &board, &flip_info);
    add_flip_constraints(&mut builder, &board, &flip_info);
    add_last_constraints(&mut builder, &board, &flip_info);
    add_before_constraints(&mut builder, &board, &flip_info);

    let result = solve_by_kissat(builder.clauses(), builder.var_count(), builder.comments());

    if verbose {
        let board_str = Board::new(player, opponent).to_string();
        println!(
            "board={}, ans={}, vars={}, clauses={}",
            board_str,
            if result { "SAT" } else { "UNSAT" },
            builder.var_count(),
            builder.clauses().len()
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_at_most_one() {
        let mut builder = ClauseBuilder::new();
        builder.at_most_one(&[1, 2, 3]);
        // C(3,2) = 3 pairs: (1,2), (1,3), (2,3)
        assert_eq!(builder.clauses().len(), 3);
        assert!(builder.clauses().contains(&vec![-2, -1]));
        assert!(builder.clauses().contains(&vec![-3, -1]));
        assert!(builder.clauses().contains(&vec![-3, -2]));
    }

    #[test]
    fn test_at_least_one() {
        let mut builder = ClauseBuilder::new();
        builder.at_least_one(&[1, 2, 3]);
        assert_eq!(builder.clauses().len(), 1);
        assert_eq!(builder.clauses()[0], vec![1, 2, 3]);
    }

    #[test]
    fn test_exactly_one() {
        let mut builder = ClauseBuilder::new();
        builder.exactly_one(&[1, 2, 3]);
        // 1 at-least-one + 3 at-most-one = 4 clauses
        assert_eq!(builder.clauses().len(), 4);
    }

    #[test]
    fn test_implies() {
        let mut builder = ClauseBuilder::new();
        builder.implies(1, 2);
        assert_eq!(builder.clauses(), &vec![vec![-1, 2]]);
    }

    #[test]
    fn test_implies_all() {
        let mut builder = ClauseBuilder::new();
        builder.implies_all(&[1, 2], 3);
        // (a1 and a2) -> b  ===  -a1 or -a2 or b
        assert_eq!(builder.clauses(), &vec![vec![-1, -2, 3]]);
    }

    #[test]
    fn test_add_antisymmetry() {
        let mut builder = ClauseBuilder::new();
        builder.add_antisymmetry(1, 2);
        assert_eq!(builder.clauses(), &vec![vec![-1, -2]]);
    }

    #[test]
    fn test_add_transitivity() {
        let mut builder = ClauseBuilder::new();
        builder.add_transitivity(1, 2, 3);
        // (a<b and b<c) -> a<c  ===  -ab or -bc or ac
        assert_eq!(builder.clauses(), &vec![vec![-1, -2, 3]]);
    }

    #[test]
    fn test_flip_id() {
        let first = FlipId::first(27, 0);
        assert!(first.is_first());
        assert_eq!(first.to_tuple(), (27, 0, 0, 0));

        let flip = FlipId::flip(10, 1, 3, 5);
        assert!(!flip.is_first());
        assert_eq!(flip.to_tuple(), (10, 1, 3, 5));

        let from_tuple = FlipId::from_tuple((10, 1, 3, 5));
        assert_eq!(from_tuple, flip);
    }

    #[test]
    fn test_board_info_initial() {
        // 初期盤面: 中央4マスのみ
        let player = 0x0000000810000000u64; // d4, e5
        let opponent = 0x0000001008000000u64; // e4, d5
        let info = BoardInfo::new(player, opponent).unwrap();
        assert_eq!(info.center_squares.len(), 4);
        assert_eq!(info.outer_squares.len(), 0);
        assert_eq!(info.all_squares.len(), 4);
    }

    #[test]
    fn test_board_info_validation() {
        // 重複チェック
        let overlapping = BoardInfo::new(0x1, 0x1);
        assert!(overlapping.is_err());

        // 中央が埋まっていない
        let no_center = BoardInfo::new(0x1, 0x2);
        assert!(no_center.is_err());
    }

    #[test]
    fn test_board_info_final_color() {
        let player = 0x0000000810000000u64;
        let opponent = 0x0000001008000000u64;
        let info = BoardInfo::new(player, opponent).unwrap();

        // player = 0x0000000810000000 = sq 28 (e4) と sq 35 (d5)
        // opponent = 0x0000001008000000 = sq 27 (d4) と sq 36 (e5)

        // sq=28 (e4) はplayerの石
        assert_eq!(info.final_color(28), 0);
        assert!(info.is_player(28));

        // sq=27 (d4) はopponentの石
        assert_eq!(info.final_color(27), 1);
        assert!(!info.is_player(27));
    }
}
