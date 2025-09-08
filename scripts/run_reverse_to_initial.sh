#!/usr/bin/env bash

set -Eeuo pipefail

# Batch runner for reverse_to_initial over result/random_play/result{N}.txt
# For each input, creates output dir result/random_play/result{N} and writes:
#  - exec.log  : stdout/stderr of the run (via tee)
#  - time.txt  : timing information for the run

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Default parallelism (1 = sequential)
JOBS=1

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
  local bin_path="$1"
  local in_path="$2"
  local quiet="${QUIET:-0}"

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

  # Ensure previous logs are removed to guarantee overwrite
  rm -f "$log_file" "$time_file"
  : > "$log_file"
  if (( quiet == 1 )); then
    echo "==> Processing $in_path -> $out_dir" >> "$log_file"
  else
    echo "==> Processing $in_path -> $out_dir" | tee -a "$log_file"
  fi

  if [[ ! -x "$bin_path" ]]; then
    echo "[ERROR] Binary not executable or missing: $bin_path" >&2
    return 2
  fi

  if ext_time_cmd=$(exists_ext_time); then
    # Use external `time` with a stable format
    if (( quiet == 1 )); then
      "$ext_time_cmd" -f $'elapsed:%E\nuser:%U\nsys:%S\nmaxrss_kb:%M' -o "$time_file" \
        bash -c $'set -o pipefail; "$1" "$2" -o "$3" >> "$4" 2>&1' _ \
        "$bin_path" "$in_path" "$out_dir" "$log_file"
    else
      "$ext_time_cmd" -f $'elapsed:%E\nuser:%U\nsys:%S\nmaxrss_kb:%M' -o "$time_file" \
        bash -c $'set -o pipefail; "$1" "$2" -o "$3" 2>&1 | tee "$4"' _ \
        "$bin_path" "$in_path" "$out_dir" "$log_file"
    fi
  else
    # Portable fallback using shell time -p (writes to stderr); capture it separately
    if (( quiet == 1 )); then
      { time -p bash -c $'set -o pipefail; "$1" "$2" -o "$3" >> "$4" 2>&1' _ \
        "$bin_path" "$in_path" "$out_dir" "$log_file"; } 2> "$time_file"
    else
      { time -p bash -c $'set -o pipefail; "$1" "$2" -o "$3" 2>&1 | tee "$4"' _ \
        "$bin_path" "$in_path" "$out_dir" "$log_file"; } 2> "$time_file"
    fi
  fi

  if (( quiet == 1 )); then
    echo "==> Done: $in_path" >> "$log_file"
  else
    echo "==> Done: $in_path" | tee -a "$log_file"
  fi
}

# Detect available CPU cores for auto parallelism
detect_cpus() {
  if command -v nproc >/dev/null 2>&1; then
    nproc
    return 0
  fi
  case "$(uname -s)" in
    Darwin)
      command -v sysctl >/dev/null 2>&1 && sysctl -n hw.ncpu && return 0
      ;;
  esac
  getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1
}

usage() {
  cat <<USAGE
Usage:
  $(basename "$0") [-j N]                    # build once, then process all result/random_play/result*.txt
  $(basename "$0") [-j N] N [N ...]          # build once, then process specified numbers (e.g., 24 25)
  $(basename "$0") [-j N] PATH [PATH ...]    # build once, then process explicit file paths

Options:
  -j, --jobs N   Run up to N jobs in parallel (default: 1)
                 Use N=0 or N=auto to match CPU cores.

Outputs per run go to random_play/result{N}/{exec.log,time.txt}.
USAGE
}

main() {
  # Special sub-command for parallel worker: args = BIN INFILE
  if [[ "${1:-}" == "__run_one" ]]; then
    shift
    run_one "$1" "$2"
    return $?
  fi

  local positional=()
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -h|--help)
        usage; return 0 ;;
      -j|--jobs)
        shift || true
        if [[ -z "${1:-}" ]]; then
          echo "Missing value for -j/--jobs" >&2; return 2
        fi
        JOBS="$1" ;;
      -j*)
        JOBS="${1#-j}" ;;
      --)
        shift; positional+=("$@"); break ;;
      *)
        positional+=("$1") ;;
    esac
    shift || true
  done

  # Normalize JOBS
  case "$JOBS" in
    0|auto|AUTO|Auto)
      JOBS="$(detect_cpus)" ;;
  esac
  if ! [[ "$JOBS" =~ ^[0-9]+$ ]] || (( JOBS < 1 )); then
    echo "Invalid jobs value: $JOBS" >&2
    return 2
  fi

  # Build input file list
  local files=()
  if [[ ${#positional[@]} -eq 0 ]]; then
    shopt -s nullglob
    mapfile -t files < <(cd "$REPO_ROOT" && printf '%s\n' result/random_play/result*.txt | sort -V)
    shopt -u nullglob
    if [[ ${#files[@]} -eq 0 ]]; then
      echo "No input files matched: $REPO_ROOT/result/random_play/result*.txt" >&2
      return 1
    fi
    # Make files absolute
    for i in "${!files[@]}"; do files[$i]="$REPO_ROOT/${files[$i]}"; done
  else
    for arg in "${positional[@]}"; do
      if [[ "$arg" =~ ^[0-9]+$ ]]; then
        files+=("$REPO_ROOT/result/random_play/result${arg}.txt")
      elif [[ -f "$arg" ]]; then
        files+=("$arg")
      elif [[ -f "$REPO_ROOT/$arg" ]]; then
        files+=("$REPO_ROOT/$arg")
      else
        echo "[WARN] Skipping unknown argument: $arg" >&2
      fi
    done
  fi

  if (( ${#files[@]} == 0 )); then
    echo "No valid inputs to process." >&2
    return 1
  fi

  echo "Scheduling ${#files[@]} job(s) with -j $JOBS" >&2

  # Build the binary once up front
  echo "Building reverse_to_initial (release)..." >&2
  (
    cd "$REPO_ROOT"
    cargo build --release --bin reverse_to_initial >&2
  )
  local BIN_PATH
  BIN_PATH="$REPO_ROOT/target/release/reverse_to_initial"
  if [[ ! -x "$BIN_PATH" && -x "$BIN_PATH.exe" ]]; then
    BIN_PATH="$BIN_PATH.exe"
  fi
  if [[ ! -x "$BIN_PATH" ]]; then
    echo "[ERROR] Built binary not found: $BIN_PATH" >&2
    return 2
  fi

  if (( JOBS == 1 )); then
    for f in "${files[@]}"; do
      run_one "$BIN_PATH" "$f"
    done
  else
    # Run in parallel using xargs; delegate to this script with a hidden subcommand.
    # Use NUL delimiters to be robust to spaces.
    printf '%s\0' "${files[@]}" | xargs -0 -n1 -P "$JOBS" env QUIET=1 bash "$0" __run_one "$BIN_PATH"
  fi
}

main "$@"
