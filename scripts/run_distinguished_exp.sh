#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/run_distinguished_exp.sh <sym_ok_file> <overall_reachable_file> <out_root>

Description:
  Run the symmetry-distinguished re-experiment:
    1. Generate a symmetry-distinguished sample S and strict_input.txt.
    2. Run strict layer-sat on strict_input.txt.
    3. Run strict parallel GBFS only on layer-sat UNKNOWN positions.

Inputs are read-only. Existing output files are not overwritten unless FORCE=1.

Environment variables:
  SEED       Deterministic sampling seed (default: 0)
  LAYER_SAT_PARALLEL_GOALS
             Number of layer-sat goals solved concurrently (default: 1)
  LAYER_SAT_TIMEOUT_SECS
             Per-instance layer-sat timeout in seconds (default: unset)
  DISCS      Forward-search meeting disc count for reverse search (default: 15)
  MAX_NODES  Maximum GBFS visited nodes per target (default: 10000000000)
  THREADS    Rayon worker threads for strict GBFS (default: 60)
  USE_LP     Enable LP pruning in strict GBFS: 1 or 0 (default: 1)
  FORCE      Overwrite outputs: 1 or 0 (default: 0)

Output layout:
  <out_root>/symmetry_distinguished/
    sample_S.txt
    strict_input.txt
    sample_map.tsv
    dropped_self_symmetric.txt
    summary.txt

  <out_root>/layer_sat_strict_out/
    layer_sat_OK.txt
    layer_sat_NG.txt
    layer_sat_UNKNOWN.txt

  <out_root>/reverse_strict_gbfs_out/
    reverse_strict_gbfs_OK.txt
    reverse_strict_gbfs_NG.txt
    reverse_strict_gbfs_UNKNOWN.txt

  <out_root>/final/
    distinguished_OK.txt
    distinguished_NG.txt
    distinguished_UNKNOWN.txt
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ "$#" -ne 3 ]]; then
  usage >&2
  exit 1
fi

sym_ok_file="$1"
overall_reachable_file="$2"
out_root="${3%/}"

seed="${SEED:-0}"
layer_sat_parallel_goals="${LAYER_SAT_PARALLEL_GOALS:-1}"
layer_sat_timeout_secs="${LAYER_SAT_TIMEOUT_SECS:-}"
discs="${DISCS:-15}"
max_nodes="${MAX_NODES:-10000000000}"
threads="${THREADS:-60}"
use_lp="${USE_LP:-1}"
force="${FORCE:-0}"

if [[ ! -f "$sym_ok_file" ]]; then
  echo "error: sym_ok file not found: $sym_ok_file" >&2
  exit 1
fi

if [[ ! -f "$overall_reachable_file" ]]; then
  echo "error: overall reachable file not found: $overall_reachable_file" >&2
  exit 1
fi

case "$use_lp" in
  0|1) ;;
  *)
    echo "error: USE_LP must be 0 or 1, got: $use_lp" >&2
    exit 1
    ;;
esac

case "$force" in
  0|1) ;;
  *)
    echo "error: FORCE must be 0 or 1, got: $force" >&2
    exit 1
    ;;
esac

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"

sample_dir="$out_root/symmetry_distinguished"
layer_sat_dir="$out_root/layer_sat_strict_out"
search_dir="$out_root/reverse_strict_gbfs_out"
final_dir="$out_root/final"
strict_input="$sample_dir/strict_input.txt"
layer_sat_unknown="$layer_sat_dir/layer_sat_UNKNOWN.txt"

symdist_bin="$repo_root/target/release/symmetry_distinguished"
layer_sat_bin="$repo_root/target/release/layer_sat"
reverse_bin="$repo_root/target/release/reverse_to_initial"

ensure_no_existing_file() {
  local path="$1"
  if [[ "$force" != "1" && -e "$path" ]]; then
    echo "error: refusing to overwrite existing file: $path" >&2
    echo "hint: choose a new out_root or rerun with FORCE=1" >&2
    exit 1
  fi
}

ensure_no_existing_file "$sample_dir/sample_S.txt"
ensure_no_existing_file "$sample_dir/strict_input.txt"
ensure_no_existing_file "$sample_dir/sample_map.tsv"
ensure_no_existing_file "$sample_dir/dropped_self_symmetric.txt"
ensure_no_existing_file "$sample_dir/summary.txt"
ensure_no_existing_file "$layer_sat_dir/layer_sat_OK.txt"
ensure_no_existing_file "$layer_sat_dir/layer_sat_NG.txt"
ensure_no_existing_file "$layer_sat_dir/layer_sat_UNKNOWN.txt"
ensure_no_existing_file "$search_dir/reverse_strict_gbfs_OK.txt"
ensure_no_existing_file "$search_dir/reverse_strict_gbfs_NG.txt"
ensure_no_existing_file "$search_dir/reverse_strict_gbfs_UNKNOWN.txt"
ensure_no_existing_file "$final_dir/distinguished_OK.txt"
ensure_no_existing_file "$final_dir/distinguished_NG.txt"
ensure_no_existing_file "$final_dir/distinguished_UNKNOWN.txt"

echo "==> Building release binaries"
cargo build --release --bin symmetry_distinguished --bin layer_sat --bin reverse_to_initial

if [[ ! -x "$symdist_bin" ]]; then
  echo "error: binary not found or not executable: $symdist_bin" >&2
  exit 1
fi
if [[ ! -x "$layer_sat_bin" ]]; then
  echo "error: binary not found or not executable: $layer_sat_bin" >&2
  exit 1
fi
if [[ ! -x "$reverse_bin" ]]; then
  echo "error: binary not found or not executable: $reverse_bin" >&2
  exit 1
fi

mkdir -p "$sample_dir" "$layer_sat_dir" "$search_dir" "$final_dir"

symdist_args=(
  --sym-ok "$sym_ok_file"
  --reachable "$overall_reachable_file"
  --out-dir "$sample_dir"
  --seed "$seed"
)
if [[ "$force" == "1" ]]; then
  symdist_args+=(--force)
fi

echo "==> Generating symmetry-distinguished sample"
"$symdist_bin" "${symdist_args[@]}"

if [[ ! -f "$strict_input" ]]; then
  echo "error: strict input was not generated: $strict_input" >&2
  exit 1
fi

if [[ ! -s "$strict_input" ]]; then
  echo "==> strict_input.txt is empty; creating empty outputs"
  : > "$layer_sat_dir/layer_sat_OK.txt"
  : > "$layer_sat_dir/layer_sat_NG.txt"
  : > "$layer_sat_dir/layer_sat_UNKNOWN.txt"
  : > "$search_dir/reverse_strict_gbfs_OK.txt"
  : > "$search_dir/reverse_strict_gbfs_NG.txt"
  : > "$search_dir/reverse_strict_gbfs_UNKNOWN.txt"
  : > "$final_dir/distinguished_OK.txt"
  : > "$final_dir/distinguished_NG.txt"
  : > "$final_dir/distinguished_UNKNOWN.txt"
  echo "Done."
  exit 0
fi

layer_sat_args=(
  --from-initial
  --goal-file "$strict_input"
  --out-dir "$layer_sat_dir"
  --parallel-goals "$layer_sat_parallel_goals"
)
if [[ -n "$layer_sat_timeout_secs" ]]; then
  layer_sat_args+=(--sat-timeout-secs "$layer_sat_timeout_secs")
fi

echo "==> Running strict layer-sat"
"$layer_sat_bin" "${layer_sat_args[@]}"

if [[ ! -f "$layer_sat_unknown" ]]; then
  echo "error: layer-sat UNKNOWN output was not generated: $layer_sat_unknown" >&2
  exit 1
fi

if [[ ! -s "$layer_sat_unknown" ]]; then
  echo "==> layer_sat_UNKNOWN.txt is empty; creating empty strict GBFS outputs"
  : > "$search_dir/reverse_strict_gbfs_OK.txt"
  : > "$search_dir/reverse_strict_gbfs_NG.txt"
  : > "$search_dir/reverse_strict_gbfs_UNKNOWN.txt"
else
  gbfs_args=(
    gbfs-strict-parallel
    --discs "$discs"
    --max-nodes "$max_nodes"
    --threads "$threads"
  )
  if [[ "$use_lp" == "1" ]]; then
    gbfs_args+=(--use-lp)
  fi
  gbfs_args+=("$layer_sat_unknown" -o "$search_dir")

  echo "==> Running strict parallel GBFS"
  "$reverse_bin" "${gbfs_args[@]}"
fi

echo "==> Writing combined final outputs"
cat "$layer_sat_dir/layer_sat_OK.txt" "$search_dir/reverse_strict_gbfs_OK.txt" \
  > "$final_dir/distinguished_OK.txt"
cat "$layer_sat_dir/layer_sat_NG.txt" "$search_dir/reverse_strict_gbfs_NG.txt" \
  > "$final_dir/distinguished_NG.txt"
cat "$search_dir/reverse_strict_gbfs_UNKNOWN.txt" \
  > "$final_dir/distinguished_UNKNOWN.txt"

echo "Done."
