#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/run_unreachable_checks.sh <input_file> <out_dir>

Description:
  Run all checks for unreachable positions.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ "$#" -ne 2 ]]; then
  usage >&2
  exit 1
fi

input_file="$1"
out_dir="$2"

if [[ ! -f "$input_file" ]]; then
  echo "error: input file not found: $input_file" >&2
  exit 1
fi

cargo build --release

mkdir -p "$out_dir"

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
check_bin="$repo_root/target/release/check"

if [[ ! -x "$check_bin" ]]; then
  echo "error: check binary not found or not executable: $check_bin" >&2
  echo "hint: run 'cargo build --release' at repository root first." >&2
  exit 1
fi

run_step() {
  local label="$1"
  local subcommand="$2"
  local in_file="$3"
  shift 3
  local expected_outputs=("$@")

  if [[ ! -f "$in_file" ]]; then
    echo "error: required input file for ${label} not found: $in_file" >&2
    exit 1
  fi

  if [[ ! -s "$in_file" ]]; then
    echo "==> ${label}: ${subcommand} (skipped: empty input)"
    for out_file in "${expected_outputs[@]}"; do
      : > "$out_file"
    done
    return 0
  fi

  echo "==> ${label}: ${subcommand}"
  "$check_bin" "$subcommand" "$in_file" -o "$out_dir"

  for out_file in "${expected_outputs[@]}"; do
    if [[ ! -f "$out_file" ]]; then
      echo "error: expected output file was not generated: $out_file" >&2
      exit 1
    fi
  done
}

sym_ok="${out_dir%/}/sym_OK.txt"
sym_ng="${out_dir%/}/sym_NG.txt"
con_ok="${out_dir%/}/con_OK.txt"
con_ng="${out_dir%/}/con_NG.txt"
edge_ok="${out_dir%/}/edge_OK.txt"
edge_ng="${out_dir%/}/edge_NG.txt"
occupancy_ok="${out_dir%/}/occupancy_OK.txt"
occupancy_ng="${out_dir%/}/occupancy_NG.txt"
occupancy_ok_ex="${out_dir%/}/occupancy_OK_explainable.txt"
occupancy_ng_ex="${out_dir%/}/occupancy_NG_explainable.txt"
seg3more_ok="${out_dir%/}/seg3more_OK.txt"
seg3more_ng="${out_dir%/}/seg3more_NG.txt"
flip_sat_ok="${out_dir%/}/flip_sat_OK.txt"
flip_sat_ng="${out_dir%/}/flip_sat_NG.txt"
lp_ok="${out_dir%/}/lp_OK.txt"
lp_ng="${out_dir%/}/lp_NG.txt"

run_step "Step 1/6" "sym" "$input_file" "$sym_ok" "$sym_ng"
run_step "Step 2/6" "con" "$sym_ok" "$con_ok" "$con_ng"
run_step "Step 3/6" "edge" "$con_ok" "$edge_ok" "$edge_ng"
run_step "Step 4/6" "occupancy" "$edge_ok" "$occupancy_ok" "$occupancy_ng" "$occupancy_ok_ex" "$occupancy_ng_ex"
run_step "Step 5/6" "seg3more" "$occupancy_ok" "$seg3more_ok" "$seg3more_ng"
run_step "Step 6/6" "lp" "$seg3more_ok" "$lp_ok" "$lp_ng"
# run_step "Step 7/7" "flip-sat" "$lp_ok" "$flip_sat_ok" "$flip_sat_ng"

# lp -> flip-sat とする理由:
# - lp -> flip-sat -> layer-sat とするほうが、SAT同士の違いが明確になりやすそう
# - flip-satよりlpのほうが軽いので、lpでNGとわかるものを先にフィルタリングしたほうが効率的

echo "Done."
