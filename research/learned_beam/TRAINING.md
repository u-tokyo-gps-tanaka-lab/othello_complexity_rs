# 前駆局面の順位学習

この学習は、合法な棋譜で観測した前駆局面を優先するための順位を作る。モデルの出力は到達可能性の確率ではなく、正誤判定にも使用しない。比較候補に付けた `observed=0` は「その元棋譜で観測していない」という意味であり、到達不能を表さない。

## 教師データ

標準初期局面から、その時点の合法手を一様に選ぶ対局を生成した。合法手がなく相手に合法手がある場合はパスし、両者に合法手がなければ終了する。元棋譜はすべて `ReachabilityRecord::verify` で再生した。別実装による配列盤面での再生も実施し、[学習用1,000棋譜](train_source_independent_verification.json)と[検証用200棋譜](validation_source_independent_verification.json)について、正確な開始局面・絶対手番・終了局面への一致を確認した。

12、16、20、24、28、32、36、40、44、48、52、56石への配置直後の局面を教師の親局面とした。実際に通った一つ前の局面を正例とし、その前に強制パスがあった場合は、同じ石数のパス前後の局面をともに既知の前駆として扱った。比較候補は、既存の必要条件を通る逆着手候補から採集した。

候補の採集では、最後に置いたマスごとの8方向の反転取り消し長を混合基数で表す。全組合せの番号から重複なしで一様に抽出するため、既存イテレータの先頭のマスや反転パターンだけを集める偏りを避けられる。抽出した配置前駆に合法な逆パス前駆があれば、それも検討する。このため、パスを含む全候補集合に対して一様な分布とは主張しない。抽出した遷移は正方向に再生し、親局面への完全一致を検査した。

各群は既知の前駆に加え、最大7個の比較候補を含む。最大128個の配置前駆を抽出する上限を設けたが、今回この上限に達した群はなかった。比較候補が一つもない群は学習から除いた。これは教師データの選択であり、探索器の判定には影響しない。

| 項目 | 学習用 | 検証用 |
|---|---:|---:|
| 元棋譜の乱数種 | 2026102501 | 2026102502 |
| 元棋譜数 | 1,000 | 200 |
| 採用した比較群 | 11,426 | 2,271 |
| 候補局面の行数 | 78,671 | 15,552 |
| 元棋譜上の前駆の行数 | 11,430 | 2,271 |
| 未ラベルの比較候補の行数 | 67,241 | 13,281 |
| 比較候補がなく除いた群 | 574 | 103 |
| 学習用との重複で除いた群 | 0 | 26 |

同じ元棋譜を学習用と検証用に分割していない。さらに、固定した初期配置を保存する恒等変換・180度回転・対角線反転2種によって局面を正規化し、親局面または候補が学習用と重なる検証群を除いた。学習プログラムでも元棋譜IDと正規化した局面集合の交差が空であることを確認する。8対称による教師の水増しは行っていない。

完全な候補CSVと正規化キー一覧は再生成できるため、モデルとともにリポジトリへは保存していない。乱数種、件数、SHA-256は [train_metadata.json](train_metadata.json) と [validation_metadata.json](validation_metadata.json) に記録している。元棋譜は [train_source_records.csv](train_source_records.csv) と [validation_source_records.csv](validation_source_records.csv) に保存した。

## 特徴量とモデル

特徴量は [`src/search/learned.rs`](../../src/search/learned.rs) の `features` が定義する160個の値で、すべて0から1の範囲である。最初の128個は手番側と相手側の石のビット平面、残り32個は絶対手番、石数、合法手数、逆着手マス数、隣接関係などを表す。モデルが保持する学習済み前処理はない。

線形モデルと、16ユニットの隠れ層を一つ持つReLUネットワークを学習した。どちらも同じ局面から単一の実数スコアを返し、大きい値を優先する。前駆集合ごとに正規化した方策確率ではない。

観測した前駆を $p$、比較候補を $c$ とすると、最小化する損失は

\[
\log\left(1+\exp\bigl(h_\theta(c)-h_\theta(p)\bigr)\right)
\]

である。群内のすべての観測前駆と比較候補の組を使用する。未知の比較候補にも合法な履歴がある可能性があり、この目的関数は元棋譜の選択を模倣するための弱い順位付けである。

NumPy 2.3.5と手書きの逆伝播を使用し、Adam、バッチサイズ1,024、重みへのL2係数0.0001で30エポック学習した。学習率は線形モデル0.003、ネットワーク0.001で固定した。BLASのスレッド数は1で、2モデルの学習全体は約12.15秒だった。線形モデルの学習種は2026102503、ネットワークは2026102519である。モデルごとに検証用の順位損失が最小だったエポックを選び、探索用の検証局面や独立評価局面は学習へ与えていない。

| モデル | 選択エポック | 検証の順位損失 | 観測前駆を比較候補より上位に置いた割合 | 観測前駆が群内1位の割合 |
|---|---:|---:|---:|---:|
| 線形 | 30 | 0.527161 | 74.08% | 41.13% |
| 隠れ16ユニット | 19 | 0.506344 | 74.28% | 40.86% |

これらは採集した比較群内の模倣成績であり、到達可能性の分類精度でも探索成功率でもない。探索中には異なる候補分布が現れるため、独立した探索ベンチマークで別に評価する必要がある。[学習曲線](models/training_curve.json)と[選択結果](models/training_metrics.json)に全エポックと石数別の値を保存した。

手書きの勾配は中心差分と照合した。線形モデル12パラメータの最大絶対誤差は約8.05×10⁻¹¹、ネットワーク36パラメータでは約1.25×10⁻¹⁰で、許容値2×10⁻⁸を下回った。[検査結果](training_gradient_verification.json)を再生成できる。RustとNumPyの特徴量・推論値の比較には、[32局面の照合データ](models/inference_parity.json)を用意した。

## 再現手順

既存の出力を上書きしないため、空の出力先を使う。

```sh
cargo build --release --locked --bin learn_reverse_ranker

target/release/learn_reverse_ranker \
  --games 1000 --seed 2026102501 --split train \
  --output /tmp/reverse-ranker-data/train.csv \
  --records-out /tmp/reverse-ranker-results/train_source_records.csv \
  --metadata-out /tmp/reverse-ranker-results/train_metadata.json \
  --keys-out /tmp/reverse-ranker-data/train_keys.txt

target/release/learn_reverse_ranker \
  --games 200 --seed 2026102502 --split validation \
  --output /tmp/reverse-ranker-data/validation.csv \
  --records-out /tmp/reverse-ranker-results/validation_source_records.csv \
  --metadata-out /tmp/reverse-ranker-results/validation_metadata.json \
  --keys-out /tmp/reverse-ranker-data/validation_keys.txt \
  --exclude-keys /tmp/reverse-ranker-data/train_keys.txt

python3 scripts/train_reverse_ranker.py \
  --train /tmp/reverse-ranker-data/train.csv \
  --validation /tmp/reverse-ranker-data/validation.csv \
  --out-dir /tmp/reverse-ranker-results/models --seed 2026102503

python3 scripts/check_reverse_ranker_gradients.py
```

実行時間を含むメタデータの全文は毎回同じにはならない。元棋譜・候補CSV・正規化キー・選択モデルの指紋を比較し、数値ライブラリや実行環境の違いによる浮動小数点差がある場合は、順位と実探索結果も確認する。
