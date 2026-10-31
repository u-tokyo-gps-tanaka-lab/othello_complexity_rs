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

抽出した局面は、計算コストの低い手法から順に3段階のパイプラインで判定します。前の段階で決着がつかなかった局面だけを次の段階へ渡します。

```text
[抽出した標本]
   │
   ▼
1. フィルタ (filters 段 / run_filters)
   └─ 到達可能局面の必要条件を検査し、違反した局面を「到達不能 (NG)」として除外する。
   │
   │ すべてのフィルタを通過した局面
   ▼
2. SAT による判定 (sat 段 / sat_reachability)
   └─ 初期局面からの着手列が存在するかを SAT ソルバ（および前処理の幅優先逆探索）で調べる。
   │
   │ 時間制限などで決着がつかなかった局面 (UNKNOWN)
   ▼
3. 逆探索による判定 (reverse_search 段 / reverse_search)
   └─ 目標局面から後向きに深さ優先探索を行い、初期局面側から生成した局面と合流するか調べる。
   │
   ▼
[最終結果: OK / NG / UNKNOWN]
```

各段階では、判定結果に応じて以下の3種類の接尾辞がついたファイルに局面を分類して出力します。ステップ1（フィルタ）とステップ2以降では `_OK` の意味が異なる点に注意してください。

| ファイル接尾辞 | フィルタ (`run_filters`) での意味 | SAT・逆探索での意味 |
|---|---|---|
| `_NG` | 到達不能が証明された。必要条件を満たさない。 | 到達不能が証明された。初期局面からの着手列が存在しない。 |
| `_OK` | 未確定。そのフィルタの必要条件を満たしたに過ぎず、到達可能の証明ではない。 | 到達可能が証明された。初期局面からの着手列が見つかり、再生検証に成功した。 |
| `_UNKNOWN` | （出力されない） | 未確定。制限時間や探索ノード数の上限に達したか、制限付きの探索で棋譜が見つからなかった。 |

### 判定モード (`--mode`)

局面の対称性や手番を同一視するかどうかを、単体コマンドでは `--mode` で指定します。推計では標本の母集団からモードが決まり、母集団の総数を厳密に計算できる `exact` または `equivalence` を使用します。

| モード | 到達可能性の判定対象 | 推計の対象 |
|---|---|---|
| `exact` | 指定された局面そのもの（1通り） | 到達可能な局面数 |
| `equivalence` | 盤面の回転・反転（8通り）と、手番・石色の同時反転（2通り）を組み合わせた最大16通りのいずれか（ゲーム理論的同値類） | 到達可能な同値類数 |
| `symmetry` | 盤面を回転・反転した最大8通りのいずれか | 利用不可（母集団の総数が未計算のため） |

`symmetry` は、単体コマンドの `sat_reachability`、`reverse_search`、`verify_record` でのみ指定できます（`run_filters` には `--mode` オプションがありません）。


## ビルド環境の準備

線形計画法ソルバ HiGHS と SAT ソルバ Kissat のソースコードはリポジトリに含まれており、`cargo build` 時に自動でコンパイルされます。`highs-sys` のビルドに `cmake` を使用するため、事前に `cmake` をインストールして PATH を通しておいてください。

```sh
cargo build --release
cargo test
```


## 到達可能局面数の推計（基本手順）

推計は、標本の抽出から信頼区間の算出までを1つの Rust プログラムとして書いて実行します。`examples/estimate.rs` は、`equivalence` モードで 1,000 局面を抽出し、到達可能な同値類数の 99.5% 信頼区間を求める例です。

```rust
use std::path::Path;
use std::time::Duration;

use othello_complexity_rs::estimate::{Pipeline, SampleSpec, SatStage};
use othello_complexity_rs::filters::sat::{ReverseBfs, SatEncoding};
use othello_complexity_rs::sampling::Population;
use othello_complexity_rs::search::run::Strategy;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report = Pipeline::sample(SampleSpec {
        population: Population::Classes,
        discs: None,
        count: 1_000,
        seed: 20261022,
    })
    .filters()
    .sat(SatStage {
        encodings: SatEncoding::Ply.into(),
        seeds: 1,
        parallel_goals: 2,
        timeout: Some(Duration::from_secs(10)),
        reverse_bfs: Some(ReverseBfs {
            discs: 10,
            state_budget: 1_000_000,
        }),
    })
    .reverse_search(Strategy::DfsParallel {
        discs: 8,
        max_nodes: 10_000,
        table_size: 100_000,
        threads: 2,
    })
    .run(Path::new("out/estimate"), 0.005)?;
    println!("{}", report.interval);
    Ok(())
}
```

この例は次のコマンドで実行できます。

```sh
cargo run --release --example estimate
```

`Pipeline::sample` から `run` までの1つの式が推計全体を表します。`filters`、`sat`、`reverse_search` の各メソッドは判定の段を宣言した順に積み上げるだけで、計算は `run` を呼んだ時点で始まります。`run` は計算を始める前に全段の設定値を検査し、不正な値があれば出力先に何も書かずにエラーで止まります。そのため、後ろの段の設定の誤りが、時間のかかる前の段を終えた後で見つかることはありません。

各段は、それより前の段で `UNKNOWN` として残った局面だけを判定します。段はどの順に何度並べてもかまいません。たとえば `sat` を2回並べると、後述の「`nopass-witness` を使った2段階の SAT 判定」になります。

`examples/estimate.rs` は書き方を示す例です。実際の推計のプログラムは、後述の「実験プログラムの置き場所」のとおり `experiments/` に置きます。

### 標本の指定

推計の起点は、新しく抽出する標本か、既存の標本ディレクトリのどちらかです。

`Pipeline::sample(SampleSpec { .. })` は、母集団から指定した数の局面を復元抽出で一様ランダムに抽出し、出力先の `sample/` に `positions.txt`、`mapping.jsonl`、`sampling.json` を書き出します。

| フィールド | 意味 |
|---|---|
| `population` | 母集団。`Population::Absolute` は `exact` モード、`Population::Classes` は `equivalence` モードに当たる。 |
| `discs` | 特定の石数の局面だけを母集団とする場合はその石数を `Some(n)` で指定し、全石数を対象とする場合は `None` とする。 |
| `count` | 抽出する局面数。 |
| `seed` | 乱数シード。`sampling.json` に記録され、同じシードを指定すれば同じ標本を再現できる。 |

`Pipeline::from_sample_dir(&Path)` は、`gen_random_boards` または `convert_boards` で作った標本ディレクトリを読み込みます。

### 判定の段

| メソッド | 判定の内容 |
|---|---|
| `filters()` | 5種類のフィルタを順に適用し、到達可能局面の必要条件を満たさない局面を `NG` とする。通過した局面は `UNKNOWN` のまま次の段へ渡る。 |
| `sat(SatStage { .. })` | SAT ソルバで初期局面からの着手列を探す。 |
| `reverse_search(Strategy::..)` | 目標局面から後向きに探索し、初期局面側から生成した局面と合流するか調べる。 |

`filters()` が適用するフィルタは以下の5つです。

| フィルタ名 | 検査する到達可能局面の必要条件 |
|---|---|
| `con` | 石のあるマスが縦・横・斜めに隣接し、1つの連結成分になっているか。 |
| `edge` | 盤の四辺と見なせる位置に、実際の対局では生じ得ない石の並びがないか。 |
| `occupancy` | 石の色を無視したとき、「既存の石が2つ並んでいる隣にしか新しい石を置けない」という緩和した着手条件のもとで、初期配置の4石から目標局面の全石まで1石ずつ配置できるか。 |
| `seg3more` | `occupancy` の条件に加え、各石を置いた時点で相手の石を挟める方向が存在し、最終的な石の色がそれらの反転結果と矛盾しないか。 |
| `lp` | 石の配置と反転関係を変数とする線形計画問題（LP）に実行可能解が存在するか。 |

`SatStage` の各フィールドの意味は次のとおりです。設定値に既定値はないので、すべてのフィールドを明示します。

| フィールド | 意味 |
|---|---|
| `encodings` | SAT への符号化方式。`SatEncoding::Ply.into()` のように1つの方式を指定するか、`SatPortfolio::new(vec![..])` で複数の方式を組み合わせる。方式は後述の「SAT 判定の詳細仕様」を参照。 |
| `seeds` | 1つの符号化方式につき、乱数シードを変えてソルバを実行する数。 |
| `parallel_goals` | 同時に判定する局面数。 |
| `timeout` | 候補局面1つあたりの SAT ソルバの制限時間。`None` なら時間制限を設けない。`equivalence` モードでは1つの入力局面につき最大16通りの対称局面を個別に解くため、1局面あたりの合計時間は指定値を超えることがある。 |
| `reverse_bfs` | SAT 前の幅優先逆探索の設定。`None` なら行わない。後述の「SAT 実行前の幅優先逆探索」を参照。 |

逆探索の方式は `Strategy` の7つの列挙子から選びます。列挙子と設定項目は、単体コマンド `reverse_search` のサブコマンドとオプションに対応します。たとえば `Strategy::DfsParallel` は `dfs-parallel` に当たります。

### 出力ディレクトリ

1回の推計の出力は、`run` に渡したディレクトリに次のようにまとまります。

```text
out/estimate/
├── sample/                 Pipeline::sample で抽出した標本
├── 01-filters/             con_OK.txt、con_NG.txt、…、lp_OK.txt、lp_NG.txt
├── 02-sat/                 sat_equivalence_OK.txt、_NG.txt、_UNKNOWN.txt、_records.csv
├── 03-reverse-search/      reverse_equivalence_OK.txt、_NG.txt、_UNKNOWN.txt、_records.csv
└── summary.json
```

段のディレクトリ名は、宣言した順番を表す2桁の番号と段の種類をつないだものです。`_records.csv` には `OK` と判定した局面への到達手順（棋譜）が入っており、`verify_record` で独立に検証できます。SAT の段は判定の終わった局面から順にファイルへ書き足すので、行の並びは入力の順とは限りません。

`summary.json` は実行ディレクトリの直下に1つだけ作られ、推計全体を記録します。標本の出どころと有意水準に加え、段ごとに設定値、完了したか否か、その段で決着した件数、その段を終えた時点の標本全体の件数と信頼区間を持ちます。`summary.json` は段が1つ終わるたびに書き直されるので、途中で止まった推計でも完了した段までの結果が残ります。

既存のディレクトリも出力先に指定できます。その場合、推計が書き出すファイルと同名のファイルだけが上書きされ、他のファイルは残ります。入力データを誤って破壊しないよう、出力先には標本ディレクトリや枝分かれ元の実行ディレクトリを指定できません。また、後述の単体コマンドの出力先には、実行ディレクトリ、その中の段のディレクトリ、実行ディレクトリの直下に新しく作るディレクトリのいずれも指定できません。

### 推計結果の見方

`run` は最後の段を終えた時点の件数と信頼区間を返します。例のプログラムは、その信頼区間を次のように表示します。

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

信頼水準は `run` の第2引数の有意水準で決まります。例の `0.005` は 99.5% 信頼区間に当たります。

### 中断した推計の再開と段の追加

`summary.json` のある実行ディレクトリに対して `run` を呼ぶと、保存済みの推計の続きから実行します。このとき `run` は、宣言した標本が保存済みのものと同じであること、および保存済みの完了した段が宣言した段の列の先頭に同じ設定値で並んでいることを確かめます。条件を満たせば、完了した段の出力を読み込んで検証し、残りの段だけを実行します。

この仕組みで、次の3つの使い方ができます。

1. 途中で止まった推計は、同じプログラムを実行し直せば再開できます。未完了だった段は最初からやり直します。ただし外部メモリ BFS の段は、`resume: true` と前回と同じ `tmp_dir` を指定すれば、その段の途中から続けられます。
2. 完了した推計に段を足したいときは、メソッドチェーンの末尾に段を加えて実行し直します。たとえば SAT の段で残った `UNKNOWN` に対して、上限を上げた逆探索を追加できます。
3. 有意水準だけを変えて実行し直すと、どの段も実行せずにすべての区間を計算し直します。

標本や完了した段の設定値が保存済みのものと異なる場合は、出力先を変更せずにエラーで止まります。最初からやり直すときは、別の出力先を指定してください。

### 途中の段からの枝分かれ

`Pipeline::from_run(&Path, k)` は、保存済みの実行ディレクトリの標本と、完了した先頭 `k` 段を引き継いだ別の推計を作ります。次の例は、`out/estimate` の SAT の段までを引き継ぎ、方式と上限を変えた逆探索を `out/estimate-gbfs` で実行します。

```rust
Pipeline::from_run(Path::new("out/estimate"), 2)
    .reverse_search(Strategy::GbfsParallel {
        discs: 10,
        max_nodes: 10_000_000,
        use_lp: false,
        threads: 20,
    })
    .run(Path::new("out/estimate-gbfs"), 0.005)?;
```

`run` は、元の実行ディレクトリにある標本と先頭 `k` 段のディレクトリを新しい出力先へ写してから、残りの段を実行します。元の実行ディレクトリは変更されず、写した後の出力先は元のディレクトリがなくても再開できます。

### 実験プログラムの置き場所

実際の推計は、実験用のクレート `experiments/` に、1つの実験につき1つのプログラムとして書きます。`experiments/src/bin/<名前>.rs` に置いたプログラムは、リポジトリのルートで次のように実行します。

```sh
cargo run --release -p experiments --bin <名前>
```

たとえば `experiments/src/bin/20261029-1m.rs` は、同値類を100万件抽出して 99.5% 信頼区間を求める推計で、`result/20261029-1m` に出力します。

`experiments` はワークスペースの別のクレートなので、ルートで実行する `cargo build` と `cargo test` の対象には入りません。実験プログラムまでまとめてコンパイルするときは `cargo build --release --workspace` を使います。

推計を終えたら、その実験プログラムを出力ディレクトリと一緒に保存します。過去の推計は、保存したプログラムとその時点のライブラリから再現するので、ライブラリの変更でコンパイルできなくなった実験プログラムは、直さずに `experiments/` から消してかまいません。ただし、`experiments/src/bin/` にプログラムが1つもないと cargo がワークスペースを読み込めず、ルートの `cargo test` も動かなくなります。そのため、プログラムは常に1つ以上置いておきます。


## SAT 判定の詳細仕様

### 符号化方式

初期局面から目標局面へ至る盤面の推移を、どのような論理式で表すかを指定します。推計では `SatStage` の `encodings` に、単体コマンド `sat_reachability` では `--encoding` に指定します。複数の方式を組み合わせると順に実行できます。推計では `SatPortfolio::new(vec![SatEncoding::NoPassWitness, SatEncoding::PlacementColumns])?` のように並べ、`--encoding` では `nopass-witness+placement-columns` のように `+` でつなぎます。

以下では用語を次のように使い分けます。
- 着手: 石を置くこと、またはパスをすること。
- 着石: 石を置くこと（パスは含まない）。
- 層: 着手列の途中の1時点における盤面を表す変数の組。
- 節: SAT の制約を構成する論理和（OR 節）の1つ。

| 方式 | 符号化の仕組み | 解なし（UNSAT）のとき |
|---|---|---|
| `ply` | 着手1回ごとに層を1つ置く。 | 到達不能（`NG`）と確定 |
| `placement` | 着石1回ごとに層を1つ置く。各着石の直前に強制パスを最大1回、最後の着石後にもパスを最大1回挟めるようにする。 | 到達不能（`NG`）と確定 |
| `placement-columns` | `placement` の式に、「初期局面で空いており目標局面で石がある各マスへ、全層を通じてちょうど1回着石する」という列方向の制約を加える。この制約は合法な着手列が必ず満たす性質を明示したもので、判定結果は変えずにソルバの探索を助ける。 | 到達不能（`NG`）と確定 |
| `placement-rays` | `placement-columns` と同等の層構成・制約を持つが、石を挟む条件を隣接マス間で共有して節数を減らす。各層の着石者を変数で表すことでパスを表現する。 | 到達不能（`NG`）と確定 |
| `nopass-witness` | パスを一度も含まず、黒白が交互に着石する着手列に限定して探索する。反転条件を隣接マス間で共有して式を小さくする。 | 未確定（`UNKNOWN`）のまま。単独実行時は `_NG` ファイルは常に空になる。 |

オセロでは一度置かれた石が盤面から取り除かれないため、目標局面で空いているマスは途中のどの時点でも空いています。すべての方式でこの性質を利用し、空きマスへの着石や反転を表す変数・節を省略して論理式を小さくしています（ただし、パスできるかどうかの判定にはそのマスへ打てるかの情報が必要なため、合法手判定の変数・節は残しています）。

### SAT 実行前の幅優先逆探索

推計の `sat` 段と単体コマンド `sat_reachability` では、SAT ソルバを呼び出す前の前処理として、目標局面から幅優先の逆探索を行います。初期局面から前向きに生成した局面表（前向き局面表）と合流するかを調べることで、浅い手数で決着がつく局面を高速に処理します。

- 前向き局面表に合流した場合: 即座に `OK`（到達可能）と確定します。得られた棋譜は、SAT の解と同様に合法手規則で再生検証されます。
- 先行局面をすべて列挙しても合流できなかった場合: 即座に `NG`（到達不能）と確定します。
- 登録局面数が上限に達した場合: 未決着として後続の SAT ソルバへ渡します。

設定項目は以下の2つです。推計では `SatStage` の `reverse_bfs` に `Some(ReverseBfs { discs, state_budget })` を指定し、`sat_reachability` ではオプションで指定します。

| 推計での指定 | `sat_reachability` のオプション | 意味 |
|---|---|---|
| `state_budget` | `--bfs-nodes`（既定値: `1000000`） | 候補局面1つあたりの逆探索で登録する局面数の上限。 |
| `discs` | `--bfs-discs`（既定値: `10`） | 合流先として使う前向き局面表の石数（4〜64）。 |

幅優先逆探索を行わない場合は、推計では `reverse_bfs: None` とし、`sat_reachability` では `--bfs-nodes 0` を指定します。

> 注意: 幅優先逆探索は到達不能（`NG`）を確定させることがあるため、到達不能を証明できる方式（`ply` や `placement` など）が符号化方式に1つ以上含まれている場合しか使えません。`nopass-witness` を単独で指定する場合は、必ず幅優先逆探索を無効にしてください。有効のままだとエラーで終了します。

### 応用例: `nopass-witness` を使った2段階の SAT 判定

`nopass-witness` はパスのある手順を見落とすため到達不能の証明（`NG` 判定）には使えませんが、パスなしで到達できる局面（`OK`）を短時間で見つけられます。これを完全な方式の前段に挟むことで、全体の実行時間を短縮できます。推計では、1つのパイプラインに `sat` の段を2つ並べます。

```rust
Pipeline::sample(spec)
    .filters()
    // 第1段階: パスなしの到達手順を1秒だけ探す。見つかった局面だけが OK になり、残りは UNKNOWN のまま次の段へ渡る
    .sat(SatStage {
        encodings: SatEncoding::NoPassWitness.into(),
        seeds: 1,
        parallel_goals: 2,
        timeout: Some(Duration::from_secs(1)),
        reverse_bfs: None, // nopass-witness 単独では幅優先逆探索を使えない
    })
    // 第2段階: 残った局面を完全な方式で判定する
    .sat(SatStage {
        encodings: SatEncoding::Ply.into(),
        seeds: 1,
        parallel_goals: 2,
        timeout: Some(Duration::from_secs(10)),
        reverse_bfs: Some(ReverseBfs { discs: 10, state_budget: 1_000_000 }),
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

局面ファイルに対して深さ優先の逆探索を実行します。

```sh
cargo run --release --bin reverse_search -- dfs-parallel \
  out/estimate/02-sat/sat_equivalence_UNKNOWN.txt \
  --mode equivalence \
  --discs 8 --max-nodes 10000 --table-size 100000 --threads 2 \
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

#### 補足: `representatives` の変換アルゴリズム

- 代表元の定義: 盤面文字列の $i$ 文字目（0始まり）を第 $i$ ビットとする64ビット整数を、「石のあるマス」と「手番側の石」のそれぞれについて作ります。盤面を回転・反転した8通りの像のうち、この2つの整数の組が最小となる盤面を代表元と定めます（上位ビットである H8 側から比較されるため、文字列として A1 側から比較する辞書順とは一致しない場合があります）。入力行に代表元でない盤面が含まれているとエラーで停止します。
- 採択・棄却サンプリング: 入力の各代表元は、回転・反転で移り合う盤面の集合を1つずつ代表しています。しかし、対称性の高い盤面ほど1つの代表元にまとまる元の盤面数が少ないため、そのまま手番を付けると自己対称な盤面が過大に標本化されてしまいます。そこで、回転・反転（8通り）× 手番（2通り）の計16通りの候補のうち、互いに異なる局面の個数を $m$（最大16）としたとき、確率 $m / 16$ でその行を採択し、$m$ 通りの中から1局面を一様に選びます。これにより偏りが補正され、手番付き局面全体からの一様標本が得られます。
