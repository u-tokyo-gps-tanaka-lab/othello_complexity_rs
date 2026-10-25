# 到達可能性ソルバの性能比較を再現する

この比較では、変更前の `9208dbd` と変更後のコードから release バイナリを別々に作成し、同じ入力と上限で実行する。`scripts/bench_reachability.py` は実行時間だけでなく、OK・NG・UNKNOWN の件数、SAT の変数数と節数、逆探索のノード数を記録する。UNKNOWN を NG に読み替えないため、時間が短くなった場合も、確定できた局面数が減っていないかを併せて確認する。

## バイナリを別の出力先で作成する

以下はリポジトリのルートで実行する。Rust 1.99.0、C/C++ コンパイラ、CMake、libclang を使える環境が必要になる。作業ツリーと Cargo の出力先には、それぞれ未使用のディレクトリを指定する。

```sh
git worktree add --detach /tmp/othello-baseline 9208dbd

CARGO_TARGET_DIR=/tmp/othello-target-baseline \
  cargo +1.99.0 build --release --locked \
  --manifest-path /tmp/othello-baseline/Cargo.toml \
  --bin sat_reachability --bin reverse_search --bin run_filters

CARGO_TARGET_DIR=/tmp/othello-target-candidate \
  cargo +1.99.0 build --release --locked \
  --bin sat_reachability --bin reverse_search --bin verify_record --bin run_filters
```

同じ `CARGO_TARGET_DIR` を両方の作業ツリーで使わない。計測プログラムは実行ファイルの SHA-256 を記録し、変更前と変更後が同一のファイルなら比較を停止する。変更後のコミット、コンパイラ、ビルド条件も、結果を報告する際に固定する。

## 過去の LP 通過局面 567 個を再現する

入力には `result/thesis_layer_sat/lp_OK.txt` を使う。このファイルは 38–58 石の相対盤面を 567 行収録し、手番を持たない。この比較では各行に ` b` を付け、黒番として判定する問い合わせを新たに定義する。以前の到達可能・不能のラベルは引き継がない。また、LP フィルタを通過したことは、到達可能性の証明ではない。

元ファイルの SHA-256 は `c0a2fd25c28616b60b92979f171eafb081a71f8135a9010c9aaf8bea17c42b70` である。次の変換は行順と重複を保ち、このハッシュと書式を確認してから出力する。

```sh
python3 - <<'PY'
from pathlib import Path
import hashlib

source = Path("result/thesis_layer_sat/lp_OK.txt")
data = source.read_bytes()
expected = "c0a2fd25c28616b60b92979f171eafb081a71f8135a9010c9aaf8bea17c42b70"
if hashlib.sha256(data).hexdigest() != expected:
    raise SystemExit("source SHA-256 mismatch")
boards = data.decode("utf-8").splitlines()
if len(boards) != 567 or any(len(b) != 64 or set(b) - set("XO-") for b in boards):
    raise SystemExit("invalid historical board file")
Path("/tmp/othello-lp-survivors-positions.txt").write_text(
    "".join(board + " b\n" for board in boards)
)
PY
```

探索順序を付けた深さ優先探索は、以下の条件で比較する。前向き探索の境界は 8 石、逆探索の上限は問い合わせごとに 10,000 ノードとし、変更前と変更後を交互の順番で 3 回ずつ実行する。

```sh
python3 scripts/bench_reachability.py compare \
  --corpus /tmp/othello-lp-survivors-positions.txt \
  --baseline /tmp/othello-target-baseline/release/reverse_search \
  --candidate /tmp/othello-target-candidate/release/reverse_search \
  --out /tmp/othello-benchmark-historical-reverse \
  --engine reverse --strategy dfs-move-ordering --mode exact \
  --forward-discs 8 --max-nodes 10000 --workers 1 --repeat 3
```

`--out` には未使用のディレクトリを指定する。各実行の標準出力、標準エラー、分類結果、証跡 CSV と、比較全体の `results.json` が保存される。確定した OK と NG の食い違いがあれば比較は失敗する。到達手順が出力された場合には、Rust のビットボード処理を参照しない Python の座標計算で全手順を再生する。このデータ集合で OK が得られない場合、その実行で検証した到達手順の本数も 0 になる。

## LP の厳密な証明書検証にかかる費用を再現する

`result/thesis_layer_sat/lp_NG.txt` の 1,024 行にも ` b` を付けて比較する。この実験は、過去に LP が棄却した入力で証明書検証の追加費用を測るものであり、過去の棄却ラベルを正解として使うものではない。次の例は元ファイルの SHA-256 を検証し、`RAYON_NUM_THREADS=1` で変更前後を交互に 3 回ずつ実行する。時間上限付き待機を使わず、各コマンドの経過時間を外から測定する。

```sh
python3 - <<'PY'
from pathlib import Path
from collections import Counter
import csv, hashlib, os, subprocess, time

source = Path("result/thesis_layer_sat/lp_NG.txt")
data = source.read_bytes()
expected = "449af5b6da050b0191e86cd71be00b0772ff1ca74cf1a908140b26f25860ef48"
if hashlib.sha256(data).hexdigest() != expected:
    raise SystemExit("source SHA-256 mismatch")
boards = data.decode("utf-8").splitlines()
if len(boards) != 1024 or any(len(b) != 64 or set(b) - set("XO-") for b in boards):
    raise SystemExit("invalid historical board file")
positions = Path("/tmp/othello-lp-ng-positions.txt")
positions.write_text("".join(board + " b\n" for board in boards))
env = dict(os.environ, RAYON_NUM_THREADS="1")
for repetition in range(1, 4):
    order = ("baseline", "candidate") if repetition % 2 else ("candidate", "baseline")
    for revision in order:
        binary = f"/tmp/othello-target-{revision}/release/run_filters"
        output = Path(f"/tmp/othello-lp-reproduction-{repetition}-{revision}")
        output.mkdir(exist_ok=False)
        started = time.perf_counter()
        with (output / "stdout.log").open("w") as log:
            subprocess.run([binary, "lp", str(positions), "--out-dir", str(output)],
                           env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
        elapsed = time.perf_counter() - started
        with (output / "lp_results.csv").open(newline="") as stream:
            counts = Counter(row["status"] for row in csv.DictReader(stream))
        print(repetition, revision, f"{elapsed:.6f} s", dict(counts))
PY
```

保存済みの結果は `results/lp-certificates.json` と `results/lp-candidate-results.csv` にある。証明書を整数演算で検証できない場合には `undetermined` として後段へ渡す。この結果は旧判定が誤りだったという証明ではなく、今回の検証で棄却を確定できなかったことを意味する。また、LP の `pass` は必要条件を満たしたという意味なので、到達可能性の OK と混同しない。

## 小規模な独立検証と難しい局面を比較する

`reachability-small.json` には 59 問を収録した。初期局面からの 45 問は、8・12・16・20・24 石の各段階について、合法対局から得た局面、石色を 1 個変更した局面、ランダムに作った局面を各 3 個ずつ含む。残る 14 問は別の開始局面を使い、強制パスと終局後の手番を検証する。合法対局から得た局面は到達可能と分かっている。8 石以下と終局付近では独立した完全列挙から期待値を求め、それ以外の色変更局面やランダム局面には未証明のラベルを付けない。

`reachability-hard.json` には 32 問を収録した。初期局面からの 18 問は、30・40・50 石の合法・色変更・ランダム局面の計 9 個と、過去の LP 通過局面から選んだ 9 個である。残る 14 問は上と同じパス・終局の検証に使う。両方のファイルは、次のコマンドで生成できる。

```sh
python3 scripts/bench_reachability.py generate \
  --out benchmarks/reachability-small.json --seed 20261026 --count 3

python3 scripts/bench_reachability.py generate \
  --out benchmarks/reachability-hard.json --seed 20261026 \
  --discs 30,40,50 --count 1 \
  --historical-lp-survivors result/thesis_layer_sat/lp_OK.txt
```

SAT の性能比較では、開始局面が標準の初期局面である問い合わせを選ぶ。次の例は小規模集合の 45 問を `placement-columns` で解く。`--encoding` を `ply` または `placement` に変更すると、ほかの符号化方式も同じ条件で比較できる。難しい集合では `--corpus` を `benchmarks/reachability-hard.json` に変更する。

```sh
python3 scripts/bench_reachability.py compare \
  --corpus benchmarks/reachability-small.json \
  --baseline /tmp/othello-target-baseline/release/sat_reachability \
  --candidate /tmp/othello-target-candidate/release/sat_reachability \
  --out /tmp/othello-benchmark-small-placement-columns \
  --engine sat --encoding placement-columns --mode exact \
  --group '^(legal|mutated|random)-|^historical-' \
  --timeout 1 --workers 1 --repeat 3
```

`--timeout 1` は、各終点候補に対する SAT ソルバ 1 回の呼出しを 1 秒に制限する。CNF の構築と読み込みはこの上限に含まれず、報告する経過時間には含まれる。`--mode equivalence` では複数の終点候補を解くため、問い合わせ全体の時間は 1 秒を超え得る。強制パスと終局を含む 59 問全体の正しさを検証したい場合は、`--group` を省略する。

## 保存された判定結果と証跡を照合する

`results/final-*.json` は最終バイナリの実行結果を保持し、各入力の分類と探索ノード数を含む。次のコマンドは、開始局面・目標局面・判定モードをキーとして、符号化方式、探索法、変更前後、反復をまたいで OK と NG が食い違わないかを確認する。UNKNOWN はこの矛盾検査では除外するが、各実行の結果には残る。

```sh
python3 scripts/bench_reachability.py check-results benchmarks/results/final-*.json
```

`results/certificates.csv` は最終 SAT 比較で得られた証跡から、6 列すべてが一致する行の重複だけを除いたファイルである。各行は開始局面、目標局面、許される変換、全着手と明示されたパスを含み、`verify_record` で再検証できる。どの実行で返された証跡かは `results/certificate_sources.json` に記録する。

```sh
/tmp/othello-target-candidate/release/verify_record \
  --input-file benchmarks/results/certificates.csv
```

## 小さい探索予算で先に確定できる局面を除く

`estimate` は、フィルタの後に逆探索を実行し、その UNKNOWN だけを SAT に渡す順序にも対応する。既に `out/filters` まで処理した標本には、例えば次のように短い探索予算を割り当てられる。各段階で確定した結果と入力の重複回数は引き継がれ、判定モードも元の標本から読み取られる。

```sh
cargo +1.99.0 run --release --locked --bin estimate -- reverse-search \
  --from out/filters --out-dir out/reverse-fast \
  dfs-move-ordering --discs 8 --max-nodes 10000

cargo +1.99.0 run --release --locked --bin estimate -- sat \
  --from out/reverse-fast --out-dir out/sat \
  --encoding placement-columns --parallel-goals 1 --sat-timeout-secs 1
```

上の SAT 設定は初回の短い予算の例である。その後も UNKNOWN である局面に限り、別の符号化方式や大きい探索予算を適用できる。必要な計算量は入力分布に依存するため、符号化方式、上限、並列数は代表的な入力で比較する。

過去の LP 通過局面 567 個を使った測定では、10,000 ノードまでの順序付き深さ優先探索で、exact モードでは 209 個、equivalence モードでは 207 個を NG と判定した。この結果から、この入力集合では逆探索を先に行うことで、それぞれ 209 個、207 個への SAT 問い合わせを省けると推論できる。ただし、段階全体の経過時間を比較した実験ではなく、別の標本に同じ件数・速度比を適用するものではない。

## 計測結果の適用範囲を確認する

計測中は別のビルドやベンチマークを同時に走らせない。計測プログラムは子プロセスの終了を待って時間を測り、非常停止用の時間上限を別のタイマーで管理する。短い処理の経過時間を歪める、タイムアウト付きプロセス待機のポーリングは使わない。

比較では、経過時間の中央値に加えて、各反復で確定した件数と UNKNOWN の件数を示す。時間上限がある実験は打ち切りを含むため、未判定の問題を解くのに本来必要な時間は分からない。ノード数が一致する逆探索では、同じ探索量を処理する費用の違いを評価できる。一方、SAT は同値な式への変形でも探索順序が変わるため、式が小さくなったことだけを高速化の証拠にはしない。

これらの入力集合は性能比較と回帰検証のためのものであり、到達可能局面全体からの一様標本ではない。速度比を別の石数、入力分布、時間上限やハードウェアへそのまま外挿しない。全テスト、到達手順の独立再生、確定結果の相互照合を組み合わせて検証するが、それらを任意の 64 マス局面に対する形式検証の完了とはみなさない。
