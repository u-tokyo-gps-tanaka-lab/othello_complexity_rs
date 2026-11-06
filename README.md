# othello_complexity.rs

Codes and raw results of the paper "Estimation of the number of legal positions in Othello" (https://ipsj.ixsq.nii.ac.jp/records/2005522 (in Japanese)), presented at IPSJ Game Programming Workshop 2025.

論文「オセロの実現可能局面数の推計」（https://ipsj.ixsq.nii.ac.jp/records/2005522 ）で使用したコードと実験結果です。論文の誤りは[正誤表](https://github.com/u-tokyo-gps-tanaka-lab/othello_complexity_rs/blob/master/%E3%82%AA%E3%82%BB%E3%83%AD%E3%81%AE%E5%AE%9F%E7%8F%BE%E5%8F%AF%E8%83%BD%E5%B1%80%E9%9D%A2%E6%95%B0%E3%81%AE%E6%8E%A8%E8%A8%88_%E6%AD%A3%E8%AA%A4%E8%A1%A8.pdf)にまとめています。


## 概要

8×8オセロにおいて、初期局面から合法手とパスの連続で到達できる局面（到達可能局面）の総数を推計するツール群です。

到達可能な局面は天文学的な数にのぼるため、すべてを数え上げることはできません。本研究では、統計的な標本抽出と到達可能性の判定アルゴリズムを組み合わせて推計します。

### 推計の手順

1. 母集団の定義: 到達可能な局面をすべて含み、かつ総数を厳密に計算できる集合を母集団と定めます。たとえば「中央4マスが黒または白で、残り60マスが空き・黒・白のいずれかであり、手番が黒または白である局面全体」を母集団とします。
2. 無作為抽出: 母集団から局面を一様ランダムに抽出します。
3. 到達可能性の判定: 抽出した各局面について、初期局面から到達できるかを判定します。
4. 区間推定: 標本中の到達可能な局面の割合から Wilson 信頼区間を求め、母集団の総数を掛けて到達可能局面数の信頼区間を算出します。

### 判定パイプラインと判定結果

抽出した局面は、計算コストの低い手法から順に4ステージのパイプラインで判定します。前のステージで決着がつかなかった局面だけを次のステージへ渡します。

```text
[抽出した標本]
   │
   ▼
1. フィルタ (Pipeline::filters / run_filters)
   └─ 到達可能局面の必要条件を検査し、違反した局面を「到達不能 (NG)」として除外する。
   │
   │ すべてのフィルタを通過した局面
   ▼
2. 幅優先逆探索による判定 (Pipeline::reverse_search / reverse_search)
   └─ 目標局面から幅優先で逆探索し、合流、探索の完了、登録局面数の上限到達のいずれかまで調べる。
   │
   │ 登録局面数の上限に達した局面 (UNKNOWN)
   ▼
3. SAT による判定 (Pipeline::sat / sat_reachability)
   └─ 初期局面からの着手列が存在するかを SAT ソルバで調べる。
   │
   │ 時間制限などで決着がつかなかった局面 (UNKNOWN)
   ▼
4. 逆探索による判定 (Pipeline::reverse_search / reverse_search)
   └─ 目標局面から後向きにグラフ探索を行い、初期局面側から生成した局面と合流するか調べる。
   │
   ▼
[最終結果: OK / NG / UNKNOWN]
```

各ステージでは、判定結果に応じて以下の3種類の接尾辞がついたファイルに局面を分類して出力します。フィルタ（`run_filters`）と SAT・逆探索では `_OK` の意味が異なる点に注意してください。

| ファイル接尾辞 | フィルタ (`run_filters`) での意味 | SAT・逆探索での意味 |
|---|---|---|
| `_NG` | 到達不能が証明された。必要条件を満たさない。 | 到達不能が証明された。初期局面からの着手列が存在しない。 |
| `_OK` | 未確定。そのフィルタの必要条件を満たしたに過ぎず、到達可能の証明ではない。 | 到達可能が証明された。初期局面からの着手列が見つかり、再生検証に成功した。 |
| `_UNKNOWN` | （出力されない） | 未確定。制限時間や探索ノード数の上限に達したか、制限付きの探索で棋譜が見つからなかった。 |

### 判定モード

局面の対称性や手番を同一視するかどうかを表す2つのモードがあります。どちらのモードも、推計に必要な母集団の総数を厳密に計算できます。

| モード | 到達可能性の判定対象 | 推計の対象 |
|---|---|---|
| `exact` | 指定された局面そのもの（1通り） | 到達可能な局面数 |
| `equivalence` | 盤面の回転・反転（8通り）と、手番・石色の同時反転（2通り）を組み合わせた最大16通りのいずれか（ゲーム理論的同値類） | 到達可能な同値類数 |

- 推計パイプラインでは、標本の母集団（`Population::Absolute` または `Population::Classes`）からモードが自動的に決まります。
- 単体コマンド（`gen_random_boards`、`sat_reachability`、`reverse_search`、`verify_record`）では `--mode` オプションで指定します。`run_filters` には `--mode` オプションがありません。


## ビルド環境の準備

線形計画法ソルバ HiGHS と SAT ソルバ Kissat のソースコードはリポジトリに含まれており、`cargo build` 時に自動でコンパイルされます。`highs-sys` のビルドに `cmake` を使用するため、事前に `cmake` をインストールして PATH を通しておいてください。

```sh
cargo build --release
cargo test
```

### メモリの設定

本リポジトリはメモリアロケータに mimalloc を使います。mimalloc は次の2つの環境変数で最大常駐メモリを減らせます。どちらも判定の手順を変える設定ではありません。

`MIMALLOC_PURGE_DELAY=0` は、解放したメモリをすぐ OS へ返す設定です。`MIMALLOC_ALLOW_THP=0` は、Linux の透過的巨大ページ (THP) をそのプロセスで無効にする設定で、大きな探索が遅くなることがあります。

どちらも `.cargo/config.toml` に書いてあるので、`cargo run` と `cargo test` では自動で有効になります。`target/release/` のバイナリを直接実行するときは有効にならないので、`MIMALLOC_PURGE_DELAY=0 MIMALLOC_ALLOW_THP=0 target/release/reverse_search ...` のように自分で指定してください。速度を優先して THP を使うときは、`MIMALLOC_ALLOW_THP=1 cargo run --release --bin reverse_search -- ...` のように指定すると、`.cargo/config.toml` の値を上書きできます。


## 到達可能局面数の推計（基本手順）

推計は、標本の抽出から信頼区間の算出までを `estimate::Pipeline` のメソッドチェーンとして記述し、1つの Rust プログラムとして実行します。

以下は、`equivalence` モードで 1,000 局面を抽出し、到達可能な同値類数の 99.5% 信頼区間を求める例（`examples/estimate.rs`）です。

```rust
use std::path::Path;
use std::time::Duration;

use othello_complexity_rs::estimate::{Pipeline, SampleSpec, SatStage};
use othello_complexity_rs::filters::sat::{SatEncoding, SatPortfolio};
use othello_complexity_rs::sampling::Population;
use othello_complexity_rs::search::Strategy;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report = Pipeline::sample(SampleSpec {
        population: Population::Classes,
        discs: None,
        count: 1_000,
        seed: 20261022,
    })
    .filters()
    .reverse_search(Strategy::Bfs {
        discs: 10,
        max_nodes: 4_000_000,
        use_lp: false,
        threads: 1,
        parallel_goals: 20,
    })
    .sat(SatStage {
        encodings: SatPortfolio::new(vec![
            SatEncoding::NoPassWitness,
            SatEncoding::PlacementRays,
        ])?,
        seeds: 1,
        parallel_goals: 10,
        timeout: Some(Duration::from_secs(120)),
    })
    .reverse_search(Strategy::GbfsParallel {
        discs: 10,
        max_nodes: 10_000_000,
        use_lp: false,
        threads: 1,
        parallel_goals: 20,
    })
    .run(Path::new("out/estimate"), 0.005)?;
    println!("{}", report.interval);
    Ok(())
}
```

この例は次のコマンドで実行できます（実際の実験プログラムは、後述の「実験プログラムの置き場所」のとおり `src/bin/` に置きます）。

```sh
cargo run --release --example estimate
```

パイプラインの動作の仕組みは次のとおりです。

- 宣言と実行の分離: `filters`、`sat`、`reverse_search` の各メソッドは判定ステージを登録するだけで、実際の計算は末尾の `run` を呼んだ時点で始まります。
- 実行前の設定検査: `run` は計算を始める前に全ステージの設定値を検査します。不正な値があれば、出力先に何も書き出さずにエラーで終了するため、途中のステージまで時間をかけて計算した後に設定ミスで止まることはありません。
- ステージ間の引き継ぎ: 各ステージは、前のステージで未確定（`UNKNOWN`）として残った局面だけを判定します。ステージの順序や回数は自由で、たとえば `sat` を2回並べると後述の「`nopass-witness` を使った2段階の SAT 判定」になります。

### 1. 標本の指定

パイプラインの起点には、新しく標本を抽出する `Pipeline::sample` か、既存の標本を読み込む `Pipeline::from_sample_dir` のいずれかを指定します。

- `Pipeline::sample(SampleSpec { .. })`: 母集団から指定した数の局面を復元抽出で一様ランダムに抽出し、出力ディレクトリの `sample/` に `positions.txt`、`mapping.jsonl`、`sampling.json` を書き出します。
- `Pipeline::from_sample_dir(&Path)`: `gen_random_boards` または `convert_boards` で作成済みの標本ディレクトリを読み込みます。

`SampleSpec` の各フィールドの意味は次のとおりです。

| フィールド | 意味 |
|---|---|
| `population` | 母集団。`Population::Absolute` は `exact` モード、`Population::Classes` は `equivalence` モードに対応する。 |
| `discs` | 特定の石数の局面だけを母集団とする場合は `Some(n)`、全石数を対象とする場合は `None`。 |
| `count` | 抽出する局面数。 |
| `seed` | 乱数シード。`sampling.json` に記録され、同じシードを指定すれば同じ標本を再現できる。 |

### 2. 判定ステージの追加

標本に対して、以下のメソッドで判定ステージを追加します。

| メソッド | 判定の内容 |
|---|---|
| `filters()` | 5種類のフィルタを順に適用し、到達可能局面の必要条件を満たさない局面を `NG` とする。通過した局面は `UNKNOWN` のまま次のステージへ渡る。 |
| `sat(SatStage { .. })` | SAT ソルバで初期局面からの着手列を探す。 |
| `reverse_search(Strategy::..)` | 目標局面から後向きに探索し、初期局面側から生成した局面と合流するか調べる。 |

#### `filters()` で適用されるフィルタ

| フィルタ名 | 検査する到達可能局面の必要条件 |
|---|---|
| `con` | 石のあるマスが縦・横・斜めに隣接し、1つの連結成分になっているか。 |
| `edge` | 盤の四辺と見なせる位置に、実際の対局では生じ得ない石の並びがないか。 |
| `occupancy` | 石の色を無視したとき、「既存の石が2つ並んでいる隣にしか新しい石を置けない」という緩和した着手条件のもとで、初期配置の4石から目標局面の全石まで1石ずつ配置できるか。 |
| `seg3more` | `occupancy` の条件に加え、各石を置いた時点で相手の石を挟める方向が存在し、最終的な石の色がそれらの反転結果と矛盾しないか。 |
| `lp` | 石の配置と反転関係を変数とする線形計画問題（LP）に実行可能解が存在するか。 |

#### `sat(SatStage { .. })` の設定項目

設定値に既定値はないため、すべてのフィールドを明示します。

| フィールド | 意味 |
|---|---|
| `encodings` | SAT への符号化方式。`SatEncoding::Ply.into()` のように1つの方式を指定するか、`SatPortfolio::new(vec![..])` で複数の方式を組み合わせる。各方式は後述の「SAT 判定の詳細仕様」を参照。 |
| `seeds` | 1つの符号化方式につき、乱数シードを変えてソルバを実行する回数。 |
| `parallel_goals` | 同時に判定する局面数。 |
| `timeout` | 候補局面1つあたりの SAT ソルバの制限時間。ポートフォリオの全ワーカーが CNF を Kissat に読み込んだ時点から測る。`None` なら制限時間を設けない。`equivalence` モードでは、標準の開始局面を固定する対称変換で候補をまとめるため、1つの入力局面につき最大4通りの対称局面を個別に解く。したがって、1局面あたりの合計時間は指定値を超えることがある。 |

#### `reverse_search(Strategy::..)` の設定項目

逆探索の方式は `Strategy` の7つの列挙子から選びます。列挙子とフィールドは、単体コマンド `reverse_search` のサブコマンドおよびオプションに対応します（例: `Strategy::DfsParallel` は `dfs-parallel` に対応）。

`Strategy::Bfs` と `Strategy::GbfsParallel` では、`threads` が1局面の探索に使うワーカースレッド数、`parallel_goals` が同時に探索する局面数です。単体コマンドの対応するオプションは `--threads` と `--parallel-goals` であり、どちらも既定値は1です。`parallel_goals` を2以上にする場合は、`threads` に1以上を指定し、両者の積を64以下にしてください。

`gbfs-parallel` の概算メモリ使用量は「`parallel_goals` × 1局面のピーク」です。探索上限が大きい場合は、1ノードあたり約90 Bが目安となります。多数の局面を処理するときは、`threads` を小さくし、利用可能な計算資源とメモリの範囲で `parallel_goals` を大きくする方が、単位時間あたりに処理できる局面数を増やしやすくなります。

### 3. 出力ディレクトリの構成と仕様

`run(out_dir, alpha)` を実行すると、`out_dir` に指定した出力ディレクトリに以下のファイル群が書き出されます。

```text
out/estimate/
├── sample/                 Pipeline::sample で抽出した標本
├── 01-filters/             con_OK.txt、con_NG.txt、…、lp_OK.txt、lp_NG.txt
├── 02-reverse-search/      reverse_equivalence_OK.txt、_NG.txt、_UNKNOWN.txt、_records.csv
├── 03-sat/                 sat_equivalence_OK.txt、_NG.txt、_UNKNOWN.txt、_records.csv
├── 04-reverse-search/      reverse_equivalence_OK.txt、_NG.txt、_UNKNOWN.txt、_records.csv
└── summary.json            推計全体の設定と各ステージの結果記録
```

- 各ステージのディレクトリ（`01-filters/` など）: 宣言順の2桁の番号とステージ種別をつないだ名前になります。SAT ステージと、`parallel_goals` が2以上の `bfs` または `gbfs-parallel` では、判定が完了した局面から順にファイルへ追記されるため、出力行の並び順は入力順とは一致しません。
- `_records.csv`: `OK`（到達可能）と判定された局面への到達手順（棋譜）が記録されます。完了順で出力される場合は、CSV の行番号ではなく `input_index` で入力局面に対応付けます。棋譜は後述の `verify_record` コマンドで独立に検証できます。
- `summary.json`: 標本の情報、有意水準（`alpha`）、および各ステージの設定値・完了状態・判定件数・そのステージ終了時点での信頼区間が記録されます。ステージが1つ完了するたびに更新されるため、実行が途中で止まっても完了したステージまでの結果が残ります。

出力ディレクトリの指定に関する注意点:

- 既存のディレクトリを指定した場合、推計が出力するファイルと同名のファイルだけが上書きされ、それ以外のファイルは残ります。
- 入力データの破損を防ぐため、標本ディレクトリや、後述する `from_run` の引き継ぎ元ディレクトリは出力先に指定できません。
- 単体コマンド（`run_filters` など）の出力先（`--out-dir`）には、推計の出力ディレクトリやその直下のサブディレクトリを指定できません。

### 4. 推計結果の見方

`run` の戻り値（`report.interval`）には、最終ステージ終了時点での集計件数と信頼区間が格納されます。`examples/estimate.rs` では次のように表示されます。

```text
Population size = 84782316550443833986427293875
Sample size = 1000
Observed reachable fraction range: [0.000000000000, 0.000000000000]
99.5% approximate Wilson CI: [0.000000000000, 0.007817838399]
Expected |R| interval: [0.000000e+0, 6.628144e+26]
```

- `Population size`: 母集団の総数。
- `Observed reachable fraction range`: 標本中に占める到達可能局面の割合の範囲。下端は未確定（`UNKNOWN`）をすべて到達不能とみなした場合、上端はすべて到達可能とみなした場合の割合です。
- `Expected |R| interval`: 到達可能局面数（`equivalence` モードでは同値類数）の信頼区間。
  - 下端: `UNKNOWN` をすべて到達不能とみなしたときの Wilson 信頼区間の下端 × 母集団の総数
  - 上端: `UNKNOWN` をすべて到達可能とみなしたときの Wilson 信頼区間の上端 × 母集団の総数

信頼水準は `run(out_dir, alpha)` の第2引数 `alpha`（有意水準）で決まります。例の `0.005` は 99.5% 信頼区間に対応します。

### 5. 中断した推計の再開とステージの追加

すでに `summary.json` が存在する出力ディレクトリに対して `run` を呼ぶと、完了済みのステージの出力ファイルを読み込んで検証し、未完了のステージから実行を再開します。現在の保存形式と一致しない結果は再開できません。

再開できるのは、プログラムで指定した標本および先頭ステージ群の設定が、保存済みの完了ステージと一致している場合です。一致しない場合は、ディレクトリを変更せずにエラーで終了します（最初からやり直す場合は別の出力ディレクトリを指定してください）。

この仕組みにより、以下の3通りの使い方が可能です。

1. 中断した推計の再開: 途中で止まったプログラムをそのまま再実行すると、未完了のステージから再開します。未完了だったステージは最初からやり直しますが、外部メモリ BFS ステージに限っては `resume: true` と前回と同じ `tmp_dir` を指定することでステージの途中から再開できます。
2. 後続ステージの追加: 完了済みの推計プログラムのメソッドチェーン末尾に新しいステージを足して再実行すると、追加したステージだけが実行されます（例: SAT ステージで残った `UNKNOWN` に対し、探索上限を引き上げた逆探索ステージを追加する）。
3. 有意水準の変更: `run` の第2引数（`alpha`）だけを変更して再実行すると、判定ステージを再実行することなく信頼区間だけを再計算します。

### 6. 途中ステージからの分岐 (`Pipeline::from_run`)

既存の推計結果の途中までを引き継ぎ、後続ステージの設定を変えた別パターンの推計を行うには `Pipeline::from_run(&Path, k)` を使います。指定した出力ディレクトリの標本と完了済みの先頭 `k` ステージを新しい出力ディレクトリへコピーした上で、続くステージを実行します。

以下の例は、`out/estimate` の第3ステージ（SAT）までの結果を引き継ぎ、逆探索の方式と上限を変えた推計を `out/estimate-gbfs` で実行します。

```rust
Pipeline::from_run(Path::new("out/estimate"), 3)
    .reverse_search(Strategy::GbfsParallel {
        discs: 10,
        max_nodes: 10_000_000,
        use_lp: false,
        threads: 1,
        parallel_goals: 20,
    })
    .run(Path::new("out/estimate-gbfs"), 0.005)?;
```

引き継ぎ元のディレクトリ（`out/estimate`）は変更されません。また、必要なファイルは新しい出力先へコピーされるため、分岐後は元のディレクトリがなくても単独で再開できます。

### 7. 実験プログラムの置き場所 (`src/bin/`)

本番の推計は、1実験あたり1バイナリとして `src/bin/<名前>.rs` に作成します。たとえば `src/bin/20261029-1m.rs` は、同値類を100万件抽出して `result/20261029-1m` に結果を出力する実験プログラムです。

`src/bin/` の直下に置いた `.rs` ファイルは Cargo が自動でバイナリとして登録するので、`Cargo.toml` を編集する必要はありません。サブディレクトリに置くと登録されないので、必ず直下に置いてください。リポジトリのルートから以下のコマンドで実行できます。

```sh
cargo run --release --bin <名前>
```

運用上の注意点:

- 推計が完了したら、再現性を保つために実験プログラム（`.rs` ファイル）を出力ディレクトリと一緒に保存してください。
- 実験プログラムは、`cargo build` や `cargo test` のたびにほかのコードと一緒にコンパイルされます。ライブラリの仕様変更で古い実験プログラムのコンパイルが通らなくなったら、その時点で修正するか削除してください。過去の実験は、保存したプログラムと当時のライブラリから再現できます。


## SAT 判定の詳細仕様

### 符号化方式

初期局面から目標局面へ至る盤面の推移を、どのような論理式で表すかを指定します。複数の方式を並べると、候補局面ごとに全方式の論理式を作って同時に解きます。再生検証を通った棋譜が見つかるか、到達不能を証明できる方式が解なしと判定した時点で、残りの求解を打ち切ります。結論は、すべてのソルバの報告を受け取り、エラーや矛盾する結果がないことを確かめてから返します。

- 推計パイプライン（`SatStage` の `encodings`）:
  - 単一方式: `SatEncoding::Ply.into()`
  - 複数方式: `SatPortfolio::new(vec![SatEncoding::NoPassWitness, SatEncoding::PlacementColumns])?`
- 単体コマンド（`sat_reachability` の `--encoding`）:
  - 単一方式: `--encoding ply`
  - 複数方式: `--encoding nopass-witness+placement-columns`（`+` で連結）

以下では用語を次のように使い分けます。
- 着手: 石を置くこと、またはパスをすること。
- 着石: 石を置くこと（パスは含まない）。
- 層: 着手列の途中の1時点における盤面を表す変数の組。
- 節: SAT の制約を構成する論理和（OR 節）の1つ。

| 方式 | 符号化の仕組み | 解なし（UNSAT）のとき |
|---|---|---|
| `ply` | 着手1回ごとに層を1つ置く。 | 到達不能（`NG`）と確定 |
| `placement-columns` | 着石1回ごとに層を1つ置く。各着石の直前に強制パスを最大1回、最後の着石後にもパスを最大1回挟めるようにする。石を挟む並びは、着石するマス、方向、挟む枚数の組ごとに列挙する。加えて、「初期局面で空いており目標局面で石がある各マスへ、全層を通じてちょうど1回着石する」という列方向の制約を課す。この制約は合法な着手列が必ず満たす性質を明示したもので、判定結果は変えずにソルバの探索を助ける。 | 到達不能（`NG`）と確定 |
| `placement-rays` | `placement-columns` と同等の層構成・制約を持つが、石を挟む条件を隣接マス間で共有して節数を減らす。各層の着石者を変数で表すことでパスを表現する。 | 到達不能（`NG`）と確定 |
| `nopass-witness` | パスを一度も含まず、黒白が交互に着石する着手列に限定して探索する。反転条件を隣接マス間で共有して式を小さくする。 | 未確定（`UNKNOWN`）のまま。単独実行時は `_NG` ファイルは常に空になる。 |

オセロでは一度置かれた石が盤面から取り除かれないため、目標局面で空いているマスは途中のどの時点でも空いています。すべての方式でこの性質を利用し、空きマスへの着石や反転を表す変数・節を省略して論理式を小さくしています（ただし、パスできるかどうかの判定にはそのマスへ打てるかの情報が必要なため、合法手判定の変数・節は残しています）。

### 幅優先逆探索

`Strategy::Bfs` は、目標局面から石数ごとに幅優先で逆探索し、初期局面側から生成した前向き局面表との合流を調べます。
前向き局面表に合流した場合は `OK` となり、復元した棋譜を合法手規則で再生検証します。
先行局面を列挙し尽くした場合は `NG`、登録局面数が `max_nodes` を超えた場合は `UNKNOWN` となります。

`discs` は前向き局面表の石数、`max_nodes` は1回の探索で登録する局面数の上限です。
`use_lp` は線形計画法による枝刈りの有無を指定し、標準の初期局面から探索するときだけ適用されます。
`threads` は1局面の層を処理するスレッド数、`parallel_goals` は同時に探索する局面数です。

`summary.json` の `nodes` には、そのステージへ入力されたすべての局面について、`OK`、`NG`、`UNKNOWN` のいずれであっても値が入力順に格納されます。
`nodes[i]` の添字 `i` は0から始まり、その値は入力順で `i + 1` 番目の局面に対応します。
フィルタの直後に実行するステージでは、入力は `01-filters/lp_OK.txt` の局面であり、そのファイルの順序が入力順になります。
この入力には重複する局面がありません。
`parallel_goals` が2以上の場合、ステージの `_OK.txt`、`_NG.txt`、`_UNKNOWN.txt` は判定の完了順に出力されるため、これらの行番号を `nodes` の添字として使うことはできません。
探索を尽くして `NG` となった局面では、対応する `nodes` がその局面を `NG` にできる最小の `max_nodes` です。
ある上限で `NG` となる局面数を求めるときは、そのステージの `_NG.txt` に記載された局面だけを入力順の位置へ対応付け、`nodes` がその上限以下である局面を数えます。

### 応用例: `nopass-witness` を使った2段階の SAT 判定

`nopass-witness` はパスのある手順を見落とすため到達不能の証明（`NG` 判定）には使えませんが、パスなしで到達できる局面（`OK`）を短時間で見つけられます。これを完全な方式の前段に挟むことで、全体の実行時間を短縮できます。推計では、1つのパイプラインに `sat` ステージを2つ並べます。

```rust
Pipeline::sample(spec)
    .filters()
    // 第1ステージ: パスなしの到達手順を1秒だけ探す。見つかった局面だけが OK になり、残りは UNKNOWN のまま次のステージへ渡る
    .sat(SatStage {
        encodings: SatEncoding::NoPassWitness.into(),
        seeds: 1,
        parallel_goals: 2,
        timeout: Some(Duration::from_secs(1)),
    })
    // 第2ステージ: 残った局面を完全な方式で判定する
    .sat(SatStage {
        encodings: SatEncoding::Ply.into(),
        seeds: 1,
        parallel_goals: 2,
        timeout: Some(Duration::from_secs(10)),
    })
    .run(Path::new("out/estimate"), 0.005)?;
```


## 単体コマンドによる個別判定

推計パイプラインを通さずに、特定の局面や局面ファイルを個別に判定・検証するコマンド群です。

### 標本の単体抽出 (`gen_random_boards`)

`Pipeline::sample` と同じ手順で標本を抽出し、標本ディレクトリに書き出します。書き出した標本は `Pipeline::from_sample_dir` で推計に使えます。`--discs` は、特定の石数の局面だけを母集団とする場合に指定します。

```sh
cargo run --release --bin gen_random_boards -- \
  --mode equivalence --count 1000 --seed 20261022 \
  --out-dir out/sample
```

### フィルタの単体実行 (`run_filters`)

指定した1種類のフィルタだけを実行します。複数のフィルタを順に適用したい場合は、出力された `_OK` ファイルを次のフィルタの入力に指定してください。

```sh
cargo run --release --bin run_filters -- con out/estimate/sample/positions.txt -o out/con
```

### SAT の単体実行 (`sat_reachability`) と棋譜の検証 (`verify_record`)

1つの局面（`--goal`）またはファイル内の局面群（`--goal-file <PATH>`）を SAT ソルバで判定します。`--from-initial` は標準の初期局面からの到達可能性を判定するフラグです。

```sh
# 1局面を直接指定して判定
cargo run --release --bin sat_reachability -- --from-initial --mode exact \
  --encoding ply \
  --goal '-------------------O-------OO------OX--------------------------- w' \
  --parallel-goals 1 --sat-timeout-secs 10 \
  --out-dir out/sat_d3

# 出力された到達手順（棋譜）が合法かどうかを独立に検証
cargo run --release --bin verify_record -- \
  --input-file out/sat_d3/sat_exact_records.csv
```

### 逆探索の単体実行 (`reverse_search`)

局面ファイルに対して逆探索を実行します。次の例では、4局面を同時に処理し、各局面の探索に1スレッドを使います。

`--max-nodes` は、`exact` では完全一致の候補ごとに、`equivalence` では入力局面ごとに適用されます。DFS では訪問した局面数、BFS と GBFS では登録した局面数を制限します。

```sh
cargo run --release --bin reverse_search -- gbfs-parallel \
  out/estimate/03-sat/sat_equivalence_UNKNOWN.txt \
  --mode equivalence \
  --discs 8 --max-nodes 10000 --threads 1 --parallel-goals 4 \
  --out-dir out/reverse_search
```


## 局面データの書式と旧データの変換

### 局面の表記形式（66文字形式）

本リポジトリの判定ツールは、盤面64文字 + 空白1文字 + 手番1文字（`b` または `w`）の計66文字で1局面を表します。

```text
---------------------------OX------XO--------------------------- b
```

- マスの並び順: A1〜H1、A2〜H2、…、A8〜H8 の順（左上から1行ずつ走査）。
- マスの記号（手番側から見た相対表記）:
  - `X`: 手番側の石
  - `O`: 相手側の石
  - `-`: 空きマス
- 手番の記号: `b`（黒番）または `w`（白番）。
- `X` と `O` は黒・白という絶対的な色ではなく、手番プレイヤーから見た相対表記です。たとえば手番が `w`（白番）の行では、`X` が白石、`O` が黒石を表します。

### 手番のない64文字データの変換 (`convert_boards`)

過去の研究データ（`result/` 内のファイルなど）には、手番情報を持たない64文字の盤面データがあります。判定ツールに入力するには、手番を補って66文字形式に変換する必要があります。

#### 個別判定だけに使う場合

推計を行わず個別の局面を判定するだけなら、そのファイルの記号の取り決めを確認した上で、行末に空白と手番を直接付与できます。たとえば各行の `X` が黒番の手番石を表しているファイルであれば、以下のコマンドで変換できます（`X` が手番側の石を表していないファイルでは、`X` と `O` の入れ替えも必要です）。

```sh
sed 's/$/ b/' boards.txt > positions.txt
```

#### 推計の標本として使う場合

母集団から一様に抽出された64文字の相対盤面データを推計の標本として使う場合は、統計的な一様性を保つために `convert_boards` で変換します。入力データの種類に応じて、2つのサブコマンドを使い分けます。

| サブコマンド | 入力データの種類 | 変換処理の内容 |
|---|---|---|
| `raw` | 回転・反転の代表元を選ぶ前の、一様抽出された64文字の相対盤面 | 各行に黒番（`b`）または白番（`w`）を等確率（1/2）で割り当てる。 |
| `representatives` | 一様抽出した64文字の相対盤面から、回転・反転の代表元だけを残した標本 | 回転・反転と手番を付与した上で、採択・棄却サンプリングを行い、手番付き局面全体からの一様標本に変換する。一部の行が棄却されるため、出力行数は入力より少なくなる。 |

```sh
# 1. 代表元を選ぶ前の相対盤面標本を変換する場合
cargo run --release --bin convert_boards -- raw \
  --input raw_boards.txt --seed 20261023 \
  --source-sampling without-replacement --provenance sampling_log.txt \
  --out-dir out/converted_raw

# 2. 代表元のみからなる相対盤面標本を変換する場合
cargo run --release --bin convert_boards -- representatives \
  --input representatives.txt --seed 20261023 \
  --source-sampling without-replacement --provenance sampling_log.txt \
  --out-dir out/converted_representatives
```

共通の引数:
- `--source-sampling`: 元の標本が復元抽出なら `with-replacement`、非復元抽出なら `without-replacement` を指定します。
- `--provenance`: 元の標本の抽出条件やシードを記録したファイル（書式不問）のパス。推計の実行時にも存在を確認するため、変換後もこのファイルを削除・移動しないでください。
- `--seed`: 変換時のサンプリングに使う乱数シード（任意、既定値: `42`）。

出力ディレクトリには以下の3つのファイルが生成され、そのまま `Pipeline::from_sample_dir` に渡して推計を開始できます。
- `positions.txt`: 変換後の局面リスト（66文字形式）。
- `mapping.jsonl`: 入力行と出力局面の対応記録（`representatives` で棄却された行も記録されます）。
- `sampling.json`: 母集団の定義（中央4マスが埋まった全局面 $2^5 \times 3^{60}$ 通り）や乱数シードなどのメタデータ。

> 注意:
> - `convert_boards` で変換した標本の推計対象は `exact` モード（到達可能な局面数）に限られます。`equivalence` モードの同値類数や、石数を固定した推計には使用できません。
> - `convert_boards` は、入力ファイルが本当に一様抽出された標本かどうかは検査できません（`representatives` が検査するのは各行が代表元であるかという点のみです）。手動で選んだ盤面などを入力してもエラーにはなりませんが、出力が一様標本にならず推計結果に偏りが生じます。
> - 入力ファイルを出力先の `positions.txt`、`mapping.jsonl`、`sampling.json` と同じファイルにはできません。シンボリックリンクとハードリンクで同じファイルを指す場合もエラーになります。

#### 補足: `representatives` の変換アルゴリズム

- 代表元の定義: 盤面文字列の $i$ 文字目（0始まり）を第 $i$ ビットとする64ビット整数を、「石のあるマス」と「手番側の石」のそれぞれについて作ります。盤面を回転・反転した8通りの像のうち、この2つの整数の組が最小となる盤面を代表元と定めます（上位ビットである H8 側から比較されるため、文字列として A1 側から比較する辞書順とは一致しない場合があります）。入力行に代表元でない盤面が含まれているとエラーで停止します。
- 採択・棄却サンプリング: 入力の各代表元は、回転・反転で移り合う盤面の集合を1つずつ代表しています。しかし、対称性の高い盤面ほど1つの代表元にまとまる元の盤面数が少ないため、そのまま手番を付けると自己対称な盤面が過大に標本化されてしまいます。そこで、回転・反転（8通り）× 手番（2通り）の計16通りの候補のうち、互いに異なる局面の個数を $m$（最大16）としたとき、確率 $m / 16$ でその行を採択し、$m$ 通りの中から1局面を一様に選びます。これにより偏りが補正され、手番付き局面全体からの一様標本が得られます。
