# othello_complexity.rs

Codes and raw results of the paper "Estimation of the number of legal positions in Othello" (https://ipsj.ixsq.nii.ac.jp/records/2005522 (in Japanese)), presented at IPSJ Game Programming Workshop 2025.

論文「オセロの実現可能局面数の推計」（https://ipsj.ixsq.nii.ac.jp/records/2005522 ）で使用したコードと実験結果です。論文の誤りは[正誤表](https://github.com/u-tokyo-gps-tanaka-lab/othello_complexity_rs/blob/master/%E3%82%AA%E3%82%BB%E3%83%AD%E3%81%AE%E5%AE%9F%E7%8F%BE%E5%8F%AF%E8%83%BD%E5%B1%80%E9%9D%A2%E6%95%B0%E3%81%AE%E6%8E%A8%E8%A8%88_%E6%AD%A3%E8%AA%A4%E8%A1%A8.pdf)にまとめています。

## ビルド

依存クレート highs-sys のビルドに cmake を使うため、事前に cmake を PATH に入れておきます。

```sh
cargo build --release
cargo test
```

## 局面の形式と判定結果

局面は、64文字の盤面に空白と手番 `b`（黒）または `w`（白）を続けて書きます。盤面の `X` は手番側の石、`O` は相手の石、`-` は空きマスです。初期局面は次のとおりです。

```text
---------------------------OX------XO--------------------------- b
```

判定結果は出力ファイル名の接尾辞で区別します。`_NG` は到達不能が証明された局面、`_UNKNOWN` は探索や SAT ソルバの上限に達して判定できなかった局面です。`run_filters` の `_OK` はフィルタで到達不能と示せなかった局面にすぎず、到達可能が証明されるのは `layer_sat` と `reverse_to_initial` が `_OK` とした場合だけです。この2つは到達経路を記録した CSV も出力し、`verify_record` で検証できます。

## 判定モード

生成と到達判定では `--mode` を必ず指定します。

- `exact` は指定した局面そのものへの到達を判定します。
- `equivalence` は、8通りの対称変換と、石の色と手番の同時交換で移り合う局面を同一視し、そのいずれかに到達できるかを判定します。論文の局面数は `equivalence` で数えたものです。
- 単独の到達判定では、手番を変えない対称変換だけを同一視する `symmetry` も使えます。

## 標本抽出から推計まで

次の例は、`equivalence` の母集団から1,000局面を抽出し、3つの段で到達判定を行って信頼区間を求めます。`estimate` の各段は出力先に存在しないディレクトリを要求し、2段目以降は前の段の出力ディレクトリを `--from` で受け取ります。

```sh
cargo run --release --bin gen_random_boards -- \
  --mode equivalence --count 1000 --seed 20261022 \
  --out-dir result/example/sample
cargo run --release --bin estimate -- filters \
  --sample result/example/sample \
  --out-dir result/example/filters --alpha 0.005
cargo run --release --bin estimate -- layer-sat \
  --from result/example/filters \
  --out-dir result/example/layer_sat --alpha 0.005 \
  --parallel-goals 2 --sat-timeout-secs 10
cargo run --release --bin estimate -- reverse-search \
  --from result/example/layer_sat \
  --out-dir result/example/reverse --alpha 0.005 \
  dfs-parallel --discs 8 --max-nodes 10000 --table-size 100000 --threads 2
```

最初の `filters` は、抽出記録 `sampling.json` を読んで母集団と標本を検証し、重複を除いた局面に `con`、`edge`、`occupancy`、`seg3more`、`lp` の各フィルタを順に適用します。続く `layer-sat` と `reverse-search` は、前の段で UNKNOWN として残った局面だけを判定し直します。この2つは任意の順で何度でも重ねられるので、上限を上げた `reverse-search` をさらに続けることもできます。

各段は判定の前に `filters` の段まで遡り、標本と全段の出力を検証し直します。そのため、前の段のディレクトリは作成時の場所に残しておく必要があります。集計では、UNKNOWN をすべて到達不能とみなした場合と、すべて到達可能とみなした場合の Wilson 区間を求め、その両端を区間とします。区間は各段の `summary.json` に記録され、標準出力にも表示されます。石数を固定した母集団から抽出するには、`gen_random_boards` に `--discs` を指定します。

`reverse-search` の方式とオプションは `reverse_to_initial` のサブコマンドと同じですが、既定値と環境変数による指定はないので、すべて明示します。外部メモリ上の幅優先探索である `bfs-external` と `bfs-external-parallel` は、作業用ファイルを `--tmp-dir` に置きます。中断した探索は、同じ `--from`、方式、オプション、`--tmp-dir` に `--resume` を加え、新しい `--out-dir` を指定して再開します。

## 個別の局面の判定

`layer_sat` は、指定した局面へ初期局面から到達できるかを判定します。次の例は、初期局面で黒が D3 に打った直後の局面を判定し、出力された経路を検証します。

```sh
cargo run --release --bin layer_sat -- --from-initial --mode exact \
  --goal '-------------------O-------OO------OX--------------------------- w' \
  --parallel-goals 1 --sat-timeout-secs 10 \
  --out-dir result/layer_sat_example
cargo run --release --bin verify_record -- \
  --input-file result/layer_sat_example/layer_sat_exact_records.csv
```

`reverse_to_initial` は、`estimate reverse-search` と同じ7方式の逆探索を任意の局面ファイルに適用します。次の例は、上の推計例で `layer-sat` の段に残った局面を判定します。分類と到達経路は出力ディレクトリに書かれ、局面ごとの探索ノード数は標準出力に表示されます。

```sh
cargo run --release --bin reverse_to_initial -- dfs-parallel \
  result/example/layer_sat/layer_sat_equivalence_UNKNOWN.txt \
  --mode equivalence \
  --discs 8 --max-nodes 10000 --table-size 100000 --threads 2 \
  --out-dir result/example/reverse_to_initial
```

`result/` と `research/` にある手番なしの旧形式データは、`convert_boards` で現在の形式に変換してから使います。
