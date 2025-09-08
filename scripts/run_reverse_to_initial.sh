#!/usr/bin/env bash

set -Eeuo pipefail

# Batch runner for reverse_to_initial over result/random_play/result{N}.txt
# For each input, creates output dir result/random_play/result{N} and writes:
#  - exec.log  : stdout/stderr of the run (via tee)
#  - time.txt  : timing information for the run

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

find_repo_root() {
  # Prefer walking up from the script location to find Cargo.toml
  local d="$script_dir"
  for _ in {1..10}; do
    if [[ -f "$d/Cargo.toml" ]]; then
      echo "$d"
      return 0
    fi
    local parent
    parent="$(dirname "$d")"
    [[ "$parent" == "$d" ]] && break
    d="$parent"
  done
  # Fallback: cargo locate-project from current dir
  if command -v cargo >/dev/null 2>&1; then
    local cargo_toml
    if cargo_toml="$(cargo locate-project --message-format=plain 2>/dev/null)"; then
      echo "$(dirname "$cargo_toml")"
      return 0
    fi
  fi
  # Last resort: current working directory
  echo "$PWD"
  return 0
}

REPO_ROOT="$(find_repo_root)"

exists_ext_time() {
  if command -v /usr/bin/time >/dev/null 2>&1; then
    echo "/usr/bin/time"
    return 0
  elif command -v gtime >/dev/null 2>&1; then
    # macOS (brew coreutils)
    echo "gtime"
    return 0
  fi
  return 1
}

run_one() {
  local in_path="$1"

  if [[ ! -f "$in_path" ]]; then
    echo "[WARN] Input not found: $in_path" >&2
    return 1
  fi

  local base
  base="${in_path##*/}"            # resultN.txt
  local stem
  stem="${base%.txt}"              # resultN
  local out_dir
  out_dir="${REPO_ROOT}/result/random_play/${stem}"    # result/random_play/resultN

  mkdir -p "$out_dir"

  local log_file time_file
  log_file="$out_dir/exec.log"
  time_file="$out_dir/time.txt"

  : > "$log_file"
  echo "==> Processing $in_path -> $out_dir" | tee -a "$log_file"

  if ext_time_cmd=$(exists_ext_time); then
    # Use external `time` with a stable format
    "$ext_time_cmd" -f $'elapsed:%E\nuser:%U\nsys:%S\nmaxrss_kb:%M' -o "$time_file" \
      bash -c $'set -o pipefail; cd "$3" && cargo run --release --bin reverse_to_initial "$1" -o "$2" 2>&1 | tee "$4"' _ \
      "$in_path" "$out_dir" "$REPO_ROOT" "$log_file"
  else
    # Portable fallback using shell time -p (writes to stderr); capture it separately
    { time -p bash -c $'set -o pipefail; cd "$3" && cargo run --release --bin reverse_to_initial "$1" -o "$2" 2>&1 | tee "$4"' _ \
      "$in_path" "$out_dir" "$REPO_ROOT" "$log_file"; } 2> "$time_file"
  fi

  echo "==> Done: $in_path" | tee -a "$log_file"
}

usage() {
  cat <<USAGE
Usage:
  $(basename "$0")              # process all result/random_play/result*.txt
  $(basename "$0") N [N ...]    # process specified numbers (e.g., 24 25)
  $(basename "$0") PATH [... ]   # process explicit file paths

Outputs per run go to random_play/result{N}/{exec.log,time.txt}.
USAGE
}

main() {
  if [[ $# -eq 0 ]]; then
    shopt -s nullglob
    mapfile -t files < <(cd "$REPO_ROOT" && printf '%s\n' result/random_play/result*.txt | sort -V)
    shopt -u nullglob
    if [[ ${#files[@]} -eq 0 ]]; then
      echo "No input files matched: $REPO_ROOT/result/random_play/result*.txt" >&2
      exit 1
    fi
    for f in "${files[@]}"; do
      run_one "$REPO_ROOT/$f"
    done
    exit 0
  fi

  for arg in "$@"; do
    if [[ "$arg" =~ ^[0-9]+$ ]]; then
      run_one "$REPO_ROOT/result/random_play/result${arg}.txt"
    elif [[ -f "$arg" ]]; then
      run_one "$arg"
    elif [[ -f "$REPO_ROOT/$arg" ]]; then
      run_one "$REPO_ROOT/$arg"
    else
      echo "[WARN] Skipping unknown argument: $arg" >&2
    fi
  done
}

main "$@"
