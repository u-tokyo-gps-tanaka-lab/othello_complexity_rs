# othello_complexity.rs 

Codes and raw results of the paper "Estimation of the number of legal positions in Othello" (https://ipsj.ixsq.nii.ac.jp/records/2005522 (in Japanese)), presented at IPSJ Game Programming Workshop 2025.

"オセロの実現可能局面数の推計" https://ipsj.ixsq.nii.ac.jp/records/2005522 で使用したコードと実験結果です。なお、資料に誤りがあったため、[正誤表](https://github.com/u-tokyo-gps-tanaka-lab/othello_complexity_rs/blob/master/%E3%82%AA%E3%82%BB%E3%83%AD%E3%81%AE%E5%AE%9F%E7%8F%BE%E5%8F%AF%E8%83%BD%E5%B1%80%E9%9D%A2%E6%95%B0%E3%81%AE%E6%8E%A8%E8%A8%88_%E6%AD%A3%E8%AA%A4%E8%A1%A8.pdf)を作成しました。

## ビルド

```
cargo build --release
```

[highs-sys](https://crates.io/crates/highs-sys/1.12.1) クレートのビルドに cmake が必要です。あらかじめ cmake の PATH が通っていることを確認してください。

## 使い方

通常の入出力は、64文字の `X/O/-` に半角空白と絶対手番 `b` または `w` を続けた形式です。
`X` は常に手番側の石を表します。
向き、絶対色、手番を区別し、重複した入力局面もそのまま扱います。

```text
---------------------------OX------XO--------------------------- b
```

`gen_random_boards` は盤面と絶対手番を独立に一様抽出し、`random_play` は初期局面からの着手とパスで決まる手番を保存します。
`rnd_to_txt` の整数は絶対局面の番号で、最下位ビットの0が黒番、1が白番を表します。
残りの桁はA1からH8の順に、中央4マスを2進数、それ以外を3進数として解釈します。
母集団の大きさは `2^5 × 3^60` です。

### 到達不能局面のチェック

```
# 連結性
# `con_{OK,NG}.txt` が生成される
$ target/release/check con ./result/absolute_positions/all.txt -o ./result/absolute_positions/

# 占有到達性
# `occupancy_{OK,NG}.txt` が生成される
$ target/release/check occupancy ./result/absolute_positions/con_OK.txt -o ./result/absolute_positions/

# 反転整合性
# `seg3more_{OK,NG}.txt` が生成される
$ target/release/check seg3more ./result/absolute_positions/occupancy_OK.txt -o ./result/absolute_positions/

# SATチェック
# `flip_sat_{OK,NG}.txt` が生成される
$ target/release/check flip-sat ./result/absolute_positions/seg3more_OK.txt -o ./result/absolute_positions/

# 線形計画法チェック
# `lp_{OK,NG}.txt` が生成される
$ target/release/check lp ./result/absolute_positions/flip_sat_OK.txt -o ./result/absolute_positions/
```


### 双方向探索

異なる複数の探索手法を実装しています。
前向き集合との一致判定には、盤面の向きと絶対手番を含めます。

スレッド並列DFSの実行例:

```
$ target/release/reverse_to_initial dfs-parallel --discs=15 --max-nodes=10000000000 --table-size=2000000000 /path/to/input.txt -o /path/to/out_dir
```

スレッド並列Greedy Best-First Searchの実行例 (LPソルバの枝刈りを有効化):

```
$ target/release/reverse_to_initial gbfs-parallel --discs=17 --max-nodes=7500000000 --use-lp /path/to/input.txt -o /path/to/out_dir
```

### SATによる到達可能性の判定

`layer_sat` は開始局面から目標局面への到達を判定し、合法な全着手と全パスを証跡として保存します。
局面は64文字の `X`、`O`、`-` に半角空白と絶対手番 `b` または `w` を続けて指定します。
`X` は手番側の石を表し、次の例は標準初期局面から黒が D3 に着手した直後の白番を調べます。

```sh
target/release/layer_sat --from-initial \
  --goal '-------------------O-------OO------OX--------------------------- w' \
  --out-dir result/layer_sat_example
target/release/verify_record \
  --input-file result/layer_sat_example/layer_sat_exact_records.tsv
```

省略時は盤面と絶対手番の完全一致を求めます。
`--check-symmetry` を指定すると、開始と終了の向きをそれぞれ独立に8通りの回転と鏡映で変えてよい問い合わせになり、目標のいずれかの対称像への到達を判定します。
分類結果は `layer_sat_exact_{OK,NG,UNKNOWN}.txt` または `layer_sat_symmetry_{OK,NG,UNKNOWN}.txt` に、検証済み証跡は同じ接頭辞の `_records.tsv` に保存します。
目標ファイル、パスの検証、時間上限などの詳細は [layer_sat の利用手順](docs/layer_sat_usage.md)を参照してください。

`check` の `_OK` は到達不能性を示せなかった局面を表します。
`layer_sat` の `_OK` は指定した一致条件での到達を証明しますが、対称性を許す場合は元の向きの到達を保証しません。
フィルタと逆探索も同じ手番付き形式を読み書きします。
逆探索は標準初期局面を起点とする完全一致を判定するため、任意の開始局面や対称性を許す問い合わせを再実行するときは、同じ条件の `layer_sat --goal-file` を使います。

### 状態数の計算

```
$ target/release/compute_ci --ok 147 --ng 999349 --unknown 504
```
