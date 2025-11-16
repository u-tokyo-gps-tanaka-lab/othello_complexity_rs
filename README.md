# othello_complexity.rs 

Codes and raw results of the paper "Estimation of the number of legal positions in Othello" (https://ipsj.ixsq.nii.ac.jp/records/2005522 (in Japanese)), presented at IPSJ Game Programming Workshop 2025.

"オセロの実現可能局面数の推計" https://ipsj.ixsq.nii.ac.jp/records/2005522 で使用したコードと実験結果です。

## ビルド

```
cargo build --release
```

[highs-sys](https://crates.io/crates/highs-sys/1.12.1)クレートのビルドに cmake が必要です。あらかじめcmakeのPATHが通っていることを確認してください。

## 使い方

### 到達不能局面のチェック

```
# 対称性
# `sym_{OK,NG}.txt` が生成される
$ target/release/check sym ./result/result_gpw2025/all.txt -o ./result/result_gpw2025/

# 連結性
# `con_{OK,NG}.txt` が生成される
$ target/release/check con ./result/result_gpw2025/sym_OK.txt -o ./result/result_gpw2025/

# 占有到達性
# `occupancy_{OK,NG}.txt` が生成される
$ target/release/check occupancy ./result/result_gpw2025/con_OK.txt -o ./result/result_gpw2025/

# 反転整合性
# `seg3more_{OK,NG}.txt` が生成される
$ target/release/check seg3-more ./result/result_gpw2025/occupancy_OK.txt -o ./result/result_gpw2025/

# SATチェック
# `sat_{OK,NG}.txt` が生成される
$ target/release/check sat ./result/result_gpw2025/seg3more_OK.txt -o ./result/result_gpw2025/

# 線形計画法チェック
# `lp_{OK,NG}.txt` が生成される
$ target/release/check lp ./result/result_gpw2025/sat_OK.txt -o ./result/result_gpw2025/
```


### 双方向探索

異なる複数の探索手法を実装しています。

スレッド並列DFSの実行例:

```
$ target/release/reverse_to_initial dfs-parallel --discs=15 --max-nodes=10000000000 --table-size=2000000000 /path/to/input.txt -o /path/to/out_dir
```

スレッド並列Greedy Best-First Searchの実行例 (LPソルバの枝刈りを有効化):

```
$ target/release/reverse_to_initial gbfs-parallel --discs=17 --max-nodes=7500000000 --use-lp /path/to/input.txt -o /path/to/out_dir
```

### 状態数の計算

```
$ target/release/compute_ci --ok 147 --ng 999349 --unknown 504
Sample size = 1000000
99.5% Wilson CI: [0.000117, 0.000727]
Expected |R| interval: [7.913957e+25, 4.928495e+26]
```
