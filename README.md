# othello_complexity.rs

Codes and raw results of the paper "Estimation of the number of legal positions in Othello" (https://ipsj.ixsq.nii.ac.jp/records/2005522 (in Japanese)), presented at IPSJ Game Programming Workshop 2025.

論文「オセロの実現可能局面数の推計」（https://ipsj.ixsq.nii.ac.jp/records/2005522 ）で使用したコードと実験結果です。論文の誤りは[正誤表](https://github.com/u-tokyo-gps-tanaka-lab/othello_complexity_rs/blob/master/%E3%82%AA%E3%82%BB%E3%83%AD%E3%81%AE%E5%AE%9F%E7%8F%BE%E5%8F%AF%E8%83%BD%E5%B1%80%E9%9D%A2%E6%95%B0%E3%81%AE%E6%8E%A8%E8%A8%88_%E6%AD%A3%E8%AA%A4%E8%A1%A8.pdf)にまとめています。

## 概要

8×8オセロにおいて、初期局面から合法手とパスの連続で到達できる局面（実現可能局面）の総数を、統計的な標本抽出と判定アルゴリズムにより推計するツール群です。

### 推計の原理
到達可能な局面は天文学的な数にのぼるため、すべてを列挙することはできません。そこで、本研究では以下の手順で推計します。

1. 母集団の定義: 到達可能な局面をすべて含む上位集合（母集団）を定義する（中央4マスに石があり、残り60マスが空き/黒/白の盤面など。大きさは厳密に計算可能）。
2. 無作為抽出: 母集団から局面を一様ランダムに抽出する。
3. 到達可能性の判定: 抽出した各局面が初期局面から到達可能かを判定する。
4. 区間推定: 到達可能な局面の割合から Wilson 信頼区間を求め、母集団の大きさを掛けて到達可能局面数 $|R|$ の信頼区間を算出する。

### 判定の3段階パイプライン
抽出した局面は、計算コストの低い順に以下の3段階で判定します。前の段階で決着がつかなかった局面（未判定）のみを次の段階へ渡します。

```text
[抽出標本]
   │
   ▼
1. フィルタ (run_filters)
   └─ 到達可能局面の必要条件を調べる。違反した局面を「到達不能（NG）」として除外。
   │ (通過した局面)
   ▼
2. SAT (sat_reachability)
   └─ 盤面推移を層状の充足可能性問題 (SAT) に符号化して解く。符号化の方式を選べる。
   │ (時間切れなどで解けなかった局面)
   ▼
3. 逆探索 (reverse_search)
   └─ 目的局面から後向きに探索し、初期局面から前向きに生成した局面と合流するか調べる。
   │
[最終結果 (OK / NG / UNKNOWN)]
```


## ビルド

線形計画法ソルバ HiGHS と SAT ソルバ Kissat はクレートに含まれており、`cargo build` 時に自動でビルドされます。なお、`highs-sys` のビルドに cmake を使用するため、事前に cmake をインストールして PATH を通しておいてください。

```sh
cargo build --release
cargo test
```


## 到達可能な局面数を推計する（実行手順）

標本抽出から信頼区間の算出までを、`estimate` サブコマンドを使って段階的に進めます。
以下の例では、同値類モード（`equivalence`）で1,000局面を抽出し、99.5% 信頼区間を算出します。

各ステップの `--out-dir` には、既存のディレクトリも指定できます。
その場合、コマンドが出力するファイルと同じ名前のファイルだけが置き換わり、その他のファイルは残ります。
前回の `sampling.json` または `summary.json` は出力を書き始める前に削除され、実行が最後まで終わった時点で新しく書かれます。
ただし、`estimate` の出力先には、そのステップが読み込むディレクトリ、つまり標本のディレクトリと前の段階の出力ディレクトリを指定できません。
また、単体で判定するコマンド（`run_filters`、`sat_reachability`、`reverse_search`）は、`summary.json` を含むディレクトリ、つまり `estimate` の出力ディレクトリには出力できません。

### ステップ 1: 標本の抽出 (`gen_random_boards`)
母集団から指定数の局面を復元抽出で無作為に抽出します。

```sh
cargo run --release --bin gen_random_boards -- \
  --mode equivalence --count 1000 --seed 20261022 \
  --out-dir out/sample
```
* `--count`: 抽出する局面数。
* `--seed`: （任意）乱数シード。同じシードなら同じ標本を再現できます。使った値は `sampling.json` に記録されます。
* `--discs`: （任意）指定した石数の局面のみを母集団とする場合に指定。

### ステップ 2: フィルタによる絞り込み (`estimate filters`)
抽出した局面に5つのフィルタを順に適用し、必要条件を満たさない局面（到達不能）を排除します。

```sh
cargo run --release --bin estimate -- filters \
  --sample out/sample \
  --out-dir out/filters
```

適用されるフィルタと検査内容：
| フィルタ | 調べる必要条件 |
|---|---|
| `con` | 石のあるマスが縦横斜めに隣接し、1つの連結成分になっているか |
| `edge` | 盤の四辺と見なせる位置に、実際の対局では生じ得ない石の並びがないか |
| `occupancy` | 石の色を無視したとき、着手規則に従う順序で石を配置できるか |
| `seg3more` | `occupancy` の条件に加え、各石を置いた時点で相手の石を返し得る方向があり、最終的な石の色が起こり得る反転と矛盾しないか |
| `lp` | 石の配置と反転関係を変数とする線形計画問題 (LP) に実行可能解があるか |

### ステップ 3: SAT による判定 (`estimate sat`)
フィルタを通過した局面に対し、SAT ソルバを用いて到達可能性を判定します。

```sh
cargo run --release --bin estimate -- sat \
  --from out/filters \
  --out-dir out/sat \
  --encoding ply \
  --parallel-goals 2 --sat-timeout-secs 10
```
* `--encoding`: SAT への符号化の方式（下表）。省略できません。
* `--parallel-goals`: 同時に判定する入力局面の数。
* `--sat-timeout-secs`: SAT ソルバ1回の呼出しあたりのタイムアウト秒数。`equivalence` モードでは1局面を最大16通りの像に分けて別々に解くため、1局面にかかる時間はこの値を超えることがあります。

符号化の方式は次の3つです。どの方式も、初期局面から目標局面へ至りうる長さの着手列をすべて表すので、ソルバが解なし（UNSAT）と答えれば到達不能が証明され、解があれば着手列が得られます。方式によって異なるのは式の大きさと解くのにかかる時間であり、時間制限で UNKNOWN になる局面は方式ごとに違うことがあります。

表の「層」は、着手列の途中の1時点の盤面を表す変数の組です。「節」は、SAT の制約を成す論理和の1つです。また、「着手」は石を置くこととパスの両方を指し、「着石」は石を置くことだけを指します。

| 方式 | 符号化の内容 |
|---|---|
| `ply` | 着手の1回ごとに層を1つ置く。 |
| `placement` | 着石の1回ごとに層を1つ置く。各着石の直前には強制パスを1回まで、最後の着石の後にもパスを1回まで挟める。 |
| `placement-columns` | `placement` に、初期局面で空いていて目標局面で埋まっている各マスへ、全層を通じてちょうど1回着石するという制約を加える。 |

石は盤面から取り除かれないので、目標局面で空いているマスは途中の局面でも常に空いています。どの方式もこの性質を使い、そのマスへの着石と、それによる反転を表す変数と節を生成せずに式を小さくしています。ただし、そのマスへ打てるかどうかはパスの可否を左右するので、合法手を表す変数と節は残しています。`placement-columns` の制約は、層を行、マスを列とする表の列ごとに課すもので、合法な着手列が必ず満たす性質を明示したものです。判定結果は変えず、ソルバの探索を助けます。

### ステップ 4: 逆探索による判定 (`estimate reverse-search`)
SAT で判定できなかった局面に対し、後向き探索を実行して最終判定を行います。

```sh
cargo run --release --bin estimate -- reverse-search \
  --from out/sat \
  --out-dir out/reverse \
  dfs-parallel --discs 8 --max-nodes 10000 --table-size 100000 --threads 2
```

### 推計結果の確認
各段階の完了時、出力ディレクトリの `summary.json` および標準出力に推計結果が表示されます。

```text
Population size = 84782316550443833986427293875
Sample size = 1000
Observed reachable fraction range: [0.000000000000, 0.001000000000]
99.5% approximate Wilson CI: [0.000000000000, 0.009699915005]
Expected |R| interval: [0.000000e+0, 8.223813e+26]
```
* Population size: 母集団の総数。
* Expected |R| interval: 到達可能な局面数（または同値類数）の信頼区間。
  * 区間の下端: 未判定 (UNKNOWN) をすべて「到達不能」とみなした Wilson 信頼区間の下端 × 母集団サイズ
  * 区間の上端: 未判定 (UNKNOWN) をすべて「到達可能」とみなした Wilson 信頼区間の上端 × 母集団サイズ
* `--alpha`: 有意水準（デフォルトは `0.005` = 99.5% 信頼区間）。


## 判定結果の仕様

出力ディレクトリには、判定結果ごとに分類された局面ファイルが出力されます。ファイル名の末尾（接尾辞）が結果を表します。

| 接尾辞 | フィルタ (`run_filters`) での意味 | SAT / 逆探索 での意味 |
|---|---|---|
| `_NG` | 到達不能が証明された（必要条件を満たさない） | 到達不能が証明された（着手列が存在しない） |
| `_OK` | 未確定（必要条件を満たしたに過ぎず、到達可能の証明ではない） | 到達可能が証明された（初期局面からの着手列が存在） |
| `_UNKNOWN` | （フィルタでは出力されない） | 未確定（時間や探索ノードの上限に達した） |


## 判定モード (`--mode`)

局面の対称性や手番をどう同一視するかを指定します。

| モード | 到達可能性の判定対象 | 推計（`estimate`）での対象 |
|---|---|---|
| `exact` | 指定された局面そのもの（1通り） | 到達可能な局面数 |
| `symmetry` | 回転・反転した8通りの局面のいずれか | 利用不可（母集団の大きさが未計算のため） |
| `equivalence` | 回転・反転＋手番・石色を同時反転した16通りの局面のいずれか（ゲーム理論的同値類） | 到達可能な同値類数 |

* 推計（`estimate`）では、母集団サイズが厳密に計算できる `exact` または `equivalence` のみを使用します。
* `symmetry` は、単体コマンドのうち `sat_reachability`、`reverse_search`、`verify_record` でのみ指定できます（`run_filters` には `--mode` がありません）。


## 単体コマンドによる個別判定

推計パイプラインを通さず、任意の局面ファイルを個別に判定することも可能です。

### フィルタ単体の実行 (`run_filters`)
指定した1つのフィルタを実行します。自動パイプライン化はされないため、順に試す場合は出力を次の入力に指定します。

```sh
cargo run --release --bin run_filters -- con out/sample/positions.txt -o out/con
```

### SAT 単体の実行 (`sat_reachability`)
特定の1局面（またはファイル内の局面）を SAT ソルバで判定します。`--from-initial` は標準の初期局面からの到達を判定する指定で、`--encoding` にはステップ 3 の表の方式名を指定します。着手手順の検証には `verify_record` を使用します。

```sh
# 局面を直接指定して判定
cargo run --release --bin sat_reachability -- --from-initial --mode exact \
  --encoding ply \
  --goal '-------------------O-------OO------OX--------------------------- w' \
  --parallel-goals 1 --sat-timeout-secs 10 \
  --out-dir out/sat_d3

# 得られた着手手順を検証
cargo run --release --bin verify_record -- \
  --input-file out/sat_d3/sat_exact_records.csv
```
※ 複数局面を判定する場合は `--goal` の代わりに `--goal-file <PATH>` を指定します。

### 逆探索単体の実行 (`reverse_search`)
後向き探索による判定を行います。

```sh
cargo run --release --bin reverse_search -- dfs-parallel \
  out/sat/sat_equivalence_UNKNOWN.txt \
  --mode equivalence \
  --discs 8 --max-nodes 10000 --table-size 100000 --threads 2 \
  --out-dir out/reverse_search
```

### 実験用のビームサーチを試す

以下のビームサーチは、評価関数と幅制限の効果を比較するための実験用機能です。独立した96局面では、GBFS が54局面を解いたのに対し、幅32の `mobility` は29局面を解きました。未解決分を GBFS に渡して成功数を保つと、生成候補数は約1.2%増えました。検証した条件では汎用的な高速化は確認できていないため、既存の探索を置き換える前に、対象の入力で成功数と探索量を比較してください。条件と生データは [ビームサーチの評価](research/beam_search/README.md) に記載しています。

`beam` は候補を有限個だけ保持する best-first ビームサーチです。初期局面へ近づいた候補、つまり石数が少ない候補を優先し、同じ石数では両者の合法手数の合計が大きい候補を優先します。各層の候補をすべて展開してから次の層へ進む方式とは異なり、有望な逆着手が見つかると、その先を先に探索します。

```sh
cargo run --release --bin reverse_search -- beam positions.txt \
  --mode exact --discs 8 --beam-width 32 --heuristic mobility \
  --max-nodes 100000 --out-dir out/beam

# ビームで未解決の局面に、既存の GBFS を適用する
cargo run --release --bin reverse_search -- gbfs-parallel \
  out/beam/reverse_exact_UNKNOWN.txt \
  --mode exact --discs 8 --max-nodes 100000 --threads 1 \
  --out-dir out/beam_then_gbfs
```

`--beam-width` は石数をまたいだ待機候補の上限で、既定値は32です。このほか、展開中の1局面と、完全な局面キーを4096件まで記憶する既展開キャッシュを使います。前向き表と既存の必要条件のキャッシュは、この幅とは別です。逆着手は遅延列挙し、手順のコピーは候補の採択時に行います。

幅制限で候補を捨てるため、ビーム単独では到達可能な局面も取りこぼします。幅制限を一度でも使った後に解を発見できなければ `UNKNOWN` とし、到達不能とは判定しません。発見した解は、初期局面から目標局面までの手順を再生して検証します。パスと絶対手番も、この検証と重複判定に含まれます。前段として使い、`UNKNOWN` だけを既存の探索に渡すことで、既存手法が解ける局面を維持できます。

`--max-nodes` の単位は方式によって異なり、どちらも各 exact 候補に適用します。ビームでは、重複や必要条件で棄却したものと逆パスを含む**生成候補数**を数え、開始局面は含めません。GBFS は従来どおり、登録した相異なる局面数を数えます。標準出力の `nodes` を方式間で直接比較せず、下記のベンチマークで共通の生成数と展開数を比較してください。

`--heuristic` では次の評価を選べます。どの評価も順位付けだけに使い、到達不能の証明には使いません。

| 値 | 同じ石数で優先する候補 |
|---|---|
| `mobility` | 両者の前向き合法手数の合計が大きい局面。既定値。 |
| `branching` | 逆着手の分岐数が少ない局面。従来の GBFS と同じ評価。 |
| `coherence` | 各色の同色隣接辺数をそれぞれ1増やし、その積が大きい局面。 |
| `continuation` | `coherence` の値に、必要条件を満たす逆着手マス数を1増やして掛けた値が大きい局面。 |
| `frontier` | 占有マスクが含まれる前向き境界の局面について、絶対色の不一致数を $m$ としたときの $2^{-m}$ の合計が大きい局面。 |

推計パイプラインでも、同じ順序で実行できます。各段階は前の段階で確定した判定を引き継ぎ、未判定の局面だけを探索します。

```sh
cargo run --release --bin estimate -- reverse-search \
  --from out/sat --out-dir out/beam \
  beam --discs 8 --max-nodes 100000 --beam-width 32 --heuristic mobility

cargo run --release --bin estimate -- reverse-search \
  --from out/beam --out-dir out/beam_then_gbfs \
  gbfs-parallel --discs 8 --max-nodes 100000 --threads 1
```

`summary.json` の `reverse_strategy_settings` に、幅、評価関数、探索上限、その上限の単位などを保存します。設定欄のない過去の出力も引き続き読み込めます。

### 探索性能を比較する

`bench_reverse_search` は、固定シードの合法対局から同じ局面集合を生成し、共有の前向き表を使って探索を比較します。生成元と発見した手順を再生して検証し、各試行の生成候補数、展開局面数、候補保持数、時間、判定を CSV に保存します。既存入力は `--input` で指定できます。

```sh
cargo run --release --bin bench_reverse_search -- \
  --seed 20261026 --samples 16 --stones 16,24,32,40,48,56 \
  --discs 8 --max-nodes 100000 --threads 1 --widths 32 --repeats 3 \
  --algorithms gbfs,best-first-branching,best-first-mobility \
  --output benchmark.csv --positions-out positions.txt \
  --source-records-out source_records.csv
python3 scripts/summarize_reverse_bench.py benchmark.csv
```

ベンチマークの `best-first-*` は上記 `beam` コマンドと同じ方式です。比較用の `beam-*` は、石数の各層で幅を制限する方式で、ライブラリでは `retrospective_layered_beam_search` として提供します。`--beam-max-nodes` を使うと、GBFS の上限を保ったままビームの生成上限だけを変更できます。集計スクリプトは独立に計測した結果から二段階のコストも算出しますが、その時間はパイプラインそのものを計測した値ではありません。

実験条件、生データ、速度と成功率の比較は [ビームサーチの評価](research/beam_search/README.md) にまとめています。


## データフォーマット

### 局面の表記形式
盤面64文字 + 空白1文字 + 手番1文字（`b` または `w`）の計66文字で表します。

```text
---------------------------OX------XO--------------------------- b
```
* 盤面順序: A1〜H1、A2〜H2、...、A8〜H8 の走査順。
* マス目の記号:
  * `X`: 手番側の石
  * `O`: 相手側の石
  * `-`: 空きマス
  * ※ `X` / `O` は石の絶対的な色ではなく、手番から見た相対表記です（白番の局面では白石が `X` になります）。
* 手番記号: `b`（黒番）、`w`（白番）。

### 手番のない盤面データの変換 (`convert_boards`)

過去のデータ（`result/` など）には、手番情報のない64文字の盤面データがあります。判定ツールは66文字の局面しか読まないため、これらは手番（`b` または `w`）を付けてから使います。

個別判定に使う場合は、ファイルごとに石の記号と手番の取り決めを確かめてから、行末に空白と手番を付けます。`X` が手番側の石を表していないファイルでは、`X` と `O` を入れ替える必要もあります。取り決めを誤ると、別の局面を判定することになります。次の例は、各行の `X` が黒番の手番側の石を表すファイルの場合です。

```sh
sed 's/$/ b/' boards.txt > positions.txt
```

一様に抽出した標本を推計に使う場合は、`convert_boards` で変換します。入力は相対盤面、つまり手番プレイヤー視点で手番側の石を `X`、相手側の石を `O`、空きマスを `-` と記録した64文字の表記です。

#### サブコマンドの選択
入力データに応じて、以下の2つから使い分けます。

| サブコマンド | 入力データ | 変換処理 |
|---|---|---|
| `raw` | 代表元を選ぶ前の、一様に抽出された相対盤面 | 各行の手番を黒番・白番から等確率（1/2）で選ぶ |
| `representatives` | 一様に抽出した相対盤面から、回転・反転の代表元だけを残した標本 | 採択・棄却サンプリングにより絶対局面の一様標本に変換 |

`raw` と `representatives` は、入力が一様に抽出されたものかどうかを検査しません。`representatives` が検査するのは、各行が代表元かどうかだけです。たとえば代表元だけを残した標本を `raw` に渡したり、手で選んだ代表元の集まりを `representatives` に渡したりしても、変換はエラーなく終わります。しかし出力は一様標本にならず、推計に偏りが生じます。

`representatives` は、手番の付与に加えて盤面を回転・反転させることがあり、一部の行を棄却します。そのため、出力の行数が入力より少なくなることがあります。

#### 実行例

`--out-dir` には既存のディレクトリも指定できます。
この場合、変換が出力する同名のファイルだけを置き換え、その他のファイルは残します。
`boards.txt`、`raw_boards.txt`、`representatives.txt`、`sampling_log.txt` は、利用者が用意するファイルの名前の例です。

1. 一様相対標本を変換する
```sh
cargo run --release --bin convert_boards -- raw \
  --input raw_boards.txt --seed 20261023 \
  --source-sampling without-replacement --provenance sampling_log.txt \
  --out-dir out/converted_raw
```

2. 代表元標本を変換する
```sh
cargo run --release --bin convert_boards -- representatives \
  --input representatives.txt --seed 20261023 \
  --source-sampling without-replacement --provenance sampling_log.txt \
  --out-dir out/converted_representatives
```

#### 抽出に関する引数
次の3つの引数は、`raw` と `representatives` のどちらでも使います。

* `--source-sampling`: 入力標本の抽出方法。復元抽出なら `with-replacement`、非復元抽出なら `without-replacement` を指定します。
* `--provenance`: 入力標本の抽出条件や乱数シードを記録した、書式を問わないファイル。`estimate` 実行時にも存在を確認するため、変換後も削除や移動をしないでください。
* `--seed`: （任意）サンプリングに用いる乱数シード。省略すると 42 を使い、使った値は `sampling.json` に記録されます。

#### 推計（`estimate`）で使う場合の注意点
`raw` と `representatives` は、母集団の定義（中央4マスが埋まった全局面 $2^5 \times 3^{60}$ 通り）と、出力が一様標本であることを `sampling.json` に記録します。`estimate` はこの記録から母集団を読み取るので、出力ディレクトリはそのまま `estimate filters --sample <DIR>` に渡して推計できます。推計対象は `exact` モードと同じく「到達可能な局面数」に限られ、`equivalence` モードの同値類数や石数固定の推計には使えません。

#### 出力ファイル
出力ディレクトリには、どちらのサブコマンドでも以下の3つのファイルが生成されます。
* `positions.txt`: 変換後の局面一覧（66文字形式）。
* `mapping.jsonl`: 入力行と出力局面の対応関係。`representatives` で棄却された行も、棄却として記録されます。
* `sampling.json`: 母集団の定義、変換方法、一様標本かどうか、乱数シードなどのメタデータ。

#### （参考）`representatives` のアルゴリズム
* 代表元の定義: 盤面の i 文字目（0始まり）を第 i ビットとする64ビット整数を、石のあるマスと手番側の石についてそれぞれ作ります。回転・反転による8通りの像のうち、この2つの整数の組を前から順に比べて最小となる盤面が代表元です。整数の比較は H8 に当たる上位ビットから進むため、64文字の文字列を A1 から比べる辞書順とは、結果が一致するとは限りません。入力行に代表元以外の盤面が含まれているとエラーで停止します。
* 採択・棄却サンプリング: 回転・反転による8通りの像と手番2通りを組み合わせた最大16通りの候補のうち、互いに異なるものの個数を $m$ としたとき、確率 $m / 16$ で採択し、その中から1つの局面を一様に選択します。入力の各代表元は回転・反転で移り合う盤面の組を1つずつ表すので、一様に抽出した入力はこの組について一様な標本です。採択・棄却によって自己対称な盤面が過大に重み付けされる偏りを補正し、この標本を絶対局面の母集団からの一様標本に変換します。
