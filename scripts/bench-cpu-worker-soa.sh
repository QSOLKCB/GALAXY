#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu

SCRIPT_DIR=$(dirname "$0")
ROOT=$(CDPATH= cd -P "$SCRIPT_DIR/.." && pwd)
cd "$ROOT"

GALAXY_CARGO=${GALAXY_CARGO:-cargo}
GALAXY_RUSTC=${GALAXY_RUSTC:-rustc}
if ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  GALAXY_CARGO=${CARGO_HOME:-$HOME/.cargo}/bin/cargo
fi
if ! command -v "$GALAXY_RUSTC" >/dev/null 2>&1; then
  GALAXY_RUSTC=${RUSTC:-rustc}
fi
if ! command -v "$GALAXY_CARGO" >/dev/null 2>&1 || ! command -v "$GALAXY_RUSTC" >/dev/null 2>&1; then
  printf '%s\n' 'GALAXY worker-local SoA benchmark requires Cargo and rustc.' >&2
  exit 1
fi

normalize_positive_decimal() {
  name=$1
  raw=$2
  maximum=$3
  normalized=$(awk -v value="$raw" -v maximum="$maximum" '
    BEGIN {
      if (value !~ /^[0-9]+$/) exit 1
      sub(/^0+/, "", value)
      if (value == "") value = "0"
      number = value + 0
      if (number < 1 || number > maximum || number != int(number)) exit 1
      printf "%.0f\n", number
    }
  ') || {
    printf '%s must be an integer in 1..=%s.\n' "$name" "$maximum" >&2
    exit 2
  }
  printf '%s\n' "$normalized"
}

validate_list() {
  name=$1
  values=$2
  maximum=$3
  test -n "$values" || {
    printf '%s must not be empty.\n' "$name" >&2
    exit 2
  }
  for value in $values; do
    normalize_positive_decimal "$name" "$value" "$maximum" >/dev/null
  done
}

LOGICAL=${GALAXY_SOA_LOGICAL:-18446744073709551615}
RESIDENT=$(normalize_positive_decimal GALAXY_SOA_RESIDENT "${GALAXY_SOA_RESIDENT:-262144}" 16777216)
FRAMES=$(normalize_positive_decimal GALAXY_SOA_FRAMES "${GALAXY_SOA_FRAMES:-8}" 100000)
REPEATS=$(normalize_positive_decimal GALAXY_SOA_REPEATS "${GALAXY_SOA_REPEATS:-3}" 25)
SEED=${GALAXY_SOA_SEED:-303}
WORKERS=${GALAXY_SOA_WORKERS:-"1 2 4 8 16 32"}
TILES=${GALAXY_SOA_TILES:-"1024 4096 16384"}
validate_list GALAXY_SOA_WORKERS "$WORKERS" 256
validate_list GALAXY_SOA_TILES "$TILES" 1048576

case "$LOGICAL" in
  ''|*[!0-9]*) printf '%s\n' 'GALAXY_SOA_LOGICAL must be an exact decimal u64.' >&2; exit 2 ;;
esac
case "$SEED" in
  ''|*[!0-9]*) printf '%s\n' 'GALAXY_SOA_SEED must be an unsigned decimal integer.' >&2; exit 2 ;;
esac

STAMP=$(date -u +%Y%m%dT%H%M%SZ)
OUTPUT=${GALAXY_SOA_OUTPUT:-runs/cpu-worker-soa-${STAMP}-$$}
MATRIX=$OUTPUT/matrix.tsv
COMPARISON=$OUTPUT/comparison.tsv
SUMMARY=$OUTPUT/summary.txt
GENERIC_TARGET=$OUTPUT/target-generic
NATIVE_TARGET=$OUTPUT/target-native

if [ -e "$OUTPUT" ]; then
  if [ ! -d "$OUTPUT" ]; then
    printf 'GALAXY_SOA_OUTPUT exists but is not a directory: %s\n' "$OUTPUT" >&2
    exit 2
  fi
  contents=$(ls -A "$OUTPUT" 2>/dev/null) || {
    printf 'cannot inspect GALAXY_SOA_OUTPUT: %s\n' "$OUTPUT" >&2
    exit 2
  }
  if [ -n "$contents" ]; then
    printf 'GALAXY_SOA_OUTPUT must be empty: %s\n' "$OUTPUT" >&2
    exit 2
  fi
else
  mkdir -p "$OUTPUT"
fi

HOST_TARGET=$("$GALAXY_RUSTC" -vV | sed -n 's/^host: //p')
if [ -z "$HOST_TARGET" ]; then
  printf '%s\n' 'could not resolve rustc host triple' >&2
  exit 1
fi
HOST_ARCH=$(uname -m)
case "$HOST_ARCH" in
  x86_64|amd64) GENERIC_CPU=x86-64 ;;
  *) GENERIC_CPU=generic ;;
esac
case "$HOST_TARGET" in
  *-windows-*) EXE_SUFFIX=.exe ;;
  *) EXE_SUFFIX= ;;
esac
GENERIC_BINARY=$GENERIC_TARGET/$HOST_TARGET/release/worker_soa_probe$EXE_SUFFIX
NATIVE_BINARY=$NATIVE_TARGET/$HOST_TARGET/release/worker_soa_probe$EXE_SUFFIX
GENERIC_RUSTFLAGS="-C target-cpu=$GENERIC_CPU"
NATIVE_RUSTFLAGS='-C target-cpu=native'

{
  printf 'date_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf 'uname=%s\n' "$(uname -a)"
  printf 'host_target=%s\n' "$HOST_TARGET"
  printf 'generic_cpu=%s\n' "$GENERIC_CPU"
  printf 'logical=%s\n' "$LOGICAL"
  printf 'resident=%s\n' "$RESIDENT"
  printf 'frames=%s\n' "$FRAMES"
  printf 'repeats=%s\n' "$REPEATS"
  printf 'workers=%s\n' "$WORKERS"
  printf 'tiles=%s\n' "$TILES"
  "$GALAXY_CARGO" --version || true
  "$GALAXY_RUSTC" -vV || true
  if [ -r /proc/cpuinfo ]; then
    grep -m1 '^model name' /proc/cpuinfo || true
    grep -m1 '^flags' /proc/cpuinfo || true
  fi
} > "$OUTPUT/host.txt"

build_variant() {
  label=$1
  target_dir=$2
  rustflags=$3
  printf '%s\n' "== build $label ($rustflags) =="
  env -u CARGO_BUILD_TARGET -u CARGO_ENCODED_RUSTFLAGS \
    CARGO_TARGET_DIR="$target_dir" \
    CARGO_PROFILE_RELEASE_STRIP=false \
    RUSTC="$GALAXY_RUSTC" \
    RUSTFLAGS="$rustflags" \
    "$GALAXY_CARGO" build --manifest-path cpu-runtime/Cargo.toml \
      --release --locked --offline --target "$HOST_TARGET" --bin worker_soa_probe
}

build_variant generic "$GENERIC_TARGET" "$GENERIC_RUSTFLAGS"
build_variant native "$NATIVE_TARGET" "$NATIVE_RUSTFLAGS"

for binary in "$GENERIC_BINARY" "$NATIVE_BINARY"; do
  if [ ! -x "$binary" ]; then
    printf 'expected worker_soa_probe binary not found: %s\n' "$binary" >&2
    exit 1
  fi
done

FIRST_TILE=$(printf '%s\n' $TILES | sed -n '1p')
printf '\n%s\n' '== parity verification =='
"$NATIVE_BINARY" verify --workers 4 --tile "$FIRST_TILE" | tee "$OUTPUT/verify.txt"

disassembly_has_body() {
  awk '
    BEGIN { found = 0 }
    /^[[:space:]]*[0-9A-Fa-f]+:/ {
      line = $0
      sub(/^[[:space:]]*[0-9A-Fa-f]+:[[:space:]]*/, "", line)
      count = split(line, fields, /[[:space:]]+/)
      for (i = 1; i <= count; i++) {
        if (fields[i] ~ /^[0-9A-Fa-f][0-9A-Fa-f]$/) continue
        if (fields[i] ~ /^[A-Za-z][A-Za-z0-9_.]*$/) {
          found = 1
          exit
        }
      }
    }
    END { exit(found ? 0 : 1) }
  ' "$1"
}

disassemble_required() {
  label=$1
  binary=$2
  asm=$OUTPUT/${label}-galaxy_worker_soa_hash_batch.asm
  evidence=$OUTPUT/${label}-isa-evidence.txt

  if ! command -v objdump >/dev/null 2>&1; then
    {
      printf 'build=%s\n' "$label"
      printf 'symbol=galaxy_worker_soa_hash_batch\n'
      printf 'evidence_available=false\n'
      printf 'reason=objdump-not-found\n'
    } > "$evidence"
    printf 'required SIMD disassembly evidence unavailable for %s: objdump not found\n' "$label" >&2
    return 1
  fi

  if objdump -d -M intel --no-show-raw-insn --disassemble=galaxy_worker_soa_hash_batch "$binary" > "$asm" 2>/dev/null; then
    :
  elif objdump -d --no-show-raw-insn --disassemble=galaxy_worker_soa_hash_batch "$binary" > "$asm" 2>/dev/null; then
    :
  elif objdump -d -M intel --disassemble=galaxy_worker_soa_hash_batch "$binary" > "$asm" 2>/dev/null; then
    :
  elif objdump -d --disassemble=galaxy_worker_soa_hash_batch "$binary" > "$asm" 2>/dev/null; then
    :
  else
    {
      printf 'build=%s\n' "$label"
      printf 'symbol=galaxy_worker_soa_hash_batch\n'
      printf 'evidence_available=false\n'
      printf 'reason=objdump-disassembly-failed\n'
    } > "$evidence"
    rm -f "$asm"
    printf 'required SIMD disassembly failed for %s\n' "$label" >&2
    return 1
  fi

  if ! disassembly_has_body "$asm"; then
    {
      printf 'build=%s\n' "$label"
      printf 'symbol=galaxy_worker_soa_hash_batch\n'
      printf 'evidence_available=false\n'
      printf 'reason=probe-symbol-body-not-decoded\n'
    } > "$evidence"
    printf 'required SIMD symbol body was not decoded for %s\n' "$label" >&2
    return 1
  fi

  {
    printf 'build=%s\n' "$label"
    printf 'symbol=galaxy_worker_soa_hash_batch\n'
    printf 'evidence_available=true\n'
    printf 'assembly=%s\n' "$asm"
  } > "$evidence"
}

disassemble_required generic "$GENERIC_BINARY"
disassemble_required native "$NATIVE_BINARY"

printf 'build\tpath\trequested_workers\teffective_workers\ttile\tmedian_ns\tbest_ns\tchecksum\tpeak_rss_kib\treceipt\n' > "$MATRIX"

json_number() {
  key=$1
  file=$2
  sed -n "s/.*\"$key\": \([0-9][0-9]*\).*/\1/p" "$file" | sed -n '1p'
}

json_string() {
  key=$1
  file=$2
  sed -n "s/.*\"$key\": \"\([^\"]*\)\".*/\1/p" "$file" | sed -n '1p'
}

json_nullable_number() {
  key=$1
  file=$2
  value=$(json_number "$key" "$file")
  if [ -n "$value" ]; then
    printf '%s\n' "$value"
  else
    printf '%s\n' 'null'
  fi
}

run_one() {
  build=$1
  path=$2
  workers=$3
  tile=$4
  binary=$5
  stem=${build}-${path}-w${workers}-t${tile}
  receipt=$OUTPUT/${stem}.json
  log=$OUTPUT/${stem}.log

  "$binary" bench \
    --path "$path" \
    --logical "$LOGICAL" \
    --resident "$RESIDENT" \
    --frames "$FRAMES" \
    --workers "$workers" \
    --tile "$tile" \
    --repeats "$REPEATS" \
    --seed "$SEED" \
    --receipt "$receipt" \
    > "$log"

  median=$(json_number median_ns "$receipt")
  best=$(json_number best_ns "$receipt")
  effective=$(json_number effective_workers "$receipt")
  checksum=$(json_string checksum "$receipt")
  rss=$(json_nullable_number peak_rss_kib "$receipt")
  if [ -z "$median" ] || [ -z "$best" ] || [ -z "$effective" ] || [ -z "$checksum" ]; then
    printf 'failed to parse receipt %s\n' "$receipt" >&2
    exit 1
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$build" "$path" "$workers" "$effective" "$tile" "$median" "$best" "$checksum" "$rss" "$receipt" >> "$MATRIX"
}

printf '\n%s\n' '== worker/tile matrix =='
case_index=0
for workers in $WORKERS; do
  for tile in $TILES; do
    if [ $((case_index % 2)) -eq 0 ]; then
      run_one generic reference "$workers" "$tile" "$GENERIC_BINARY"
      run_one generic soa "$workers" "$tile" "$GENERIC_BINARY"
      run_one native reference "$workers" "$tile" "$NATIVE_BINARY"
      run_one native soa "$workers" "$tile" "$NATIVE_BINARY"
    else
      run_one native soa "$workers" "$tile" "$NATIVE_BINARY"
      run_one native reference "$workers" "$tile" "$NATIVE_BINARY"
      run_one generic soa "$workers" "$tile" "$GENERIC_BINARY"
      run_one generic reference "$workers" "$tile" "$GENERIC_BINARY"
    fi
    case_index=$((case_index + 1))
  done
done

checksum_count=$(tail -n +2 "$MATRIX" | cut -f8 | sort -u | wc -l | tr -d ' ')
if [ "$checksum_count" -ne 1 ]; then
  printf '%s\n' 'reference/SoA or generic/native checksum mismatch in matrix' >&2
  tail -n +2 "$MATRIX" | cut -f1-8 >&2
  exit 1
fi
CHECKSUM=$(tail -n +2 "$MATRIX" | cut -f8 | sed -n '1p')

awk -F '\t' '
  NR == 1 { next }
  {
    key = $3 SUBSEP $5
    if ($1 == "generic" && $2 == "reference") gr[key] = $6
    if ($1 == "generic" && $2 == "soa") gs[key] = $6
    if ($1 == "native" && $2 == "reference") nr[key] = $6
    if ($1 == "native" && $2 == "soa") ns[key] = $6
    if ($1 == "generic" && $2 == "reference") grss[key] = $9
    if ($1 == "generic" && $2 == "soa") gsrss[key] = $9
    if ($1 == "native" && $2 == "reference") nrss[key] = $9
    if ($1 == "native" && $2 == "soa") nsrss[key] = $9
    workers[key] = $3
    tiles[key] = $5
  }
  END {
    print "workers\ttile\tg_ref_ns\tg_soa_ns\tg_algorithm_speedup\tn_ref_ns\tn_soa_ns\tn_algorithm_speedup\ttotal_speedup\tnative_gain_within_soa\tg_ref_rss_kib\tg_soa_rss_kib\tn_ref_rss_kib\tn_soa_rss_kib"
    for (key in workers) {
      if (!(key in gr) || !(key in gs) || !(key in nr) || !(key in ns)) continue
      printf "%s\t%s\t%s\t%s\t%.9f\t%s\t%s\t%.9f\t%.9f\t%.9f\t%s\t%s\t%s\t%s\n", \
        workers[key], tiles[key], gr[key], gs[key], gr[key] / gs[key], \
        nr[key], ns[key], nr[key] / ns[key], gr[key] / ns[key], gs[key] / ns[key], \
        grss[key], gsrss[key], nrss[key], nsrss[key]
    }
  }
' "$MATRIX" | { IFS= read -r header; printf '%s\n' "$header"; sort -n -k1,1 -k2,2; } > "$COMPARISON"

{
  printf 'schema=galaxy.cpu-worker-soa-summary.v1\n'
  printf 'checksum=%s\n' "$CHECKSUM"
  printf 'resident=%s\n' "$RESIDENT"
  printf 'frames=%s\n' "$FRAMES"
  printf 'repeats_per_case=%s\n' "$REPEATS"
  printf 'matrix=%s\n' "$MATRIX"
  printf 'comparison=%s\n' "$COMPARISON"
  printf 'generic_isa_evidence=%s\n' "$OUTPUT/generic-isa-evidence.txt"
  printf 'native_isa_evidence=%s\n' "$OUTPUT/native-isa-evidence.txt"
  printf '%s\n' 'decision_boundary=Do not promote the SoA path solely from this script; require exact parity, repeatable end-to-end timing wins across useful worker counts, and no unacceptable RSS regression.'
} > "$SUMMARY"

printf '\n%s\n' 'GALAXY worker-local SoA sweep completed.'
printf 'output=%s\n' "$OUTPUT"
printf 'matrix=%s\n' "$MATRIX"
printf 'comparison=%s\n' "$COMPARISON"
printf 'summary=%s\n' "$SUMMARY"
printf 'checksum=%s\n' "$CHECKSUM"