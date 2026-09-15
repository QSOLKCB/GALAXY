#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu

SCRIPT_DIR=$(dirname "$0")
ROOT=$(CDPATH= cd -P "$SCRIPT_DIR/.." && pwd)
cd "$ROOT"

GALAXY_CARGO=${GALAXY_CARGO:-cargo}
if ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  GALAXY_CARGO=${CARGO_HOME:-$HOME/.cargo}/bin/cargo
fi
if [ ! -x "$GALAXY_CARGO" ] && ! command -v "$GALAXY_CARGO" >/dev/null 2>&1; then
  printf '%s\n' "GALAXY SIMD probe: Rust/Cargo is required." >&2
  exit 1
fi

ITEMS=${GALAXY_SIMD_ITEMS:-4194304}
PAIRS=${GALAXY_SIMD_REPEATS:-7}
case "$PAIRS" in
  ''|*[!0-9]*)
    printf '%s\n' "GALAXY_SIMD_REPEATS must be an integer in 1..=25." >&2
    exit 2
    ;;
esac
if [ "$PAIRS" -lt 1 ] || [ "$PAIRS" -gt 25 ]; then
  printf '%s\n' "GALAXY_SIMD_REPEATS must be in 1..=25." >&2
  exit 2
fi

STAMP=$(date -u +%Y%m%dT%H%M%SZ)
OUTPUT=${GALAXY_SIMD_OUTPUT:-runs/cpu-simd-${STAMP}-$$}
GENERIC_TARGET=$OUTPUT/target-generic
NATIVE_TARGET=$OUTPUT/target-native
GENERIC_SAMPLES=$OUTPUT/generic-samples.tsv
NATIVE_SAMPLES=$OUTPUT/native-samples.tsv
ORDER_LOG=$OUTPUT/sampling-order.tsv
GENERIC_SUMMARY=$OUTPUT/generic.json
NATIVE_SUMMARY=$OUTPUT/native.json
COMPARISON=$OUTPUT/comparison.txt

HOST_ARCH=$(uname -m)
case "$HOST_ARCH" in
  x86_64|amd64) GENERIC_CPU=x86-64 ;;
  *) GENERIC_CPU=generic ;;
esac
GENERIC_RUSTFLAGS="-C target-cpu=$GENERIC_CPU"
NATIVE_RUSTFLAGS='-C target-cpu=native'

mkdir -p "$OUTPUT"
: > "$GENERIC_SAMPLES"
: > "$NATIVE_SAMPLES"
printf 'pair\tsequence\tvariant\n' > "$ORDER_LOG"

{
  printf 'date_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf 'uname=%s\n' "$(uname -a)"
  printf 'host_arch=%s\n' "$HOST_ARCH"
  printf 'generic_rustflags=%s\n' "$GENERIC_RUSTFLAGS"
  printf 'native_rustflags=%s\n' "$NATIVE_RUSTFLAGS"
  "$GALAXY_CARGO" --version || true
  if command -v rustc >/dev/null 2>&1; then
    rustc --version -v || true
  fi
  if [ -r /proc/cpuinfo ]; then
    grep -m1 '^model name' /proc/cpuinfo || true
    grep -m1 '^flags' /proc/cpuinfo || true
  fi
} > "$OUTPUT/host.txt"

build_variant() {
  label=$1
  target_dir=$2
  rustflags=$3
  printf '%s\n' "== GALAXY SIMD probe: $label build ($rustflags) =="
  # CARGO_ENCODED_RUSTFLAGS has higher precedence than RUSTFLAGS. Remove it and
  # set RUSTFLAGS explicitly so an inherited native flag cannot contaminate the
  # generic baseline (or vice versa).
  env -u CARGO_ENCODED_RUSTFLAGS \
    CARGO_TARGET_DIR="$target_dir" \
    RUSTFLAGS="$rustflags" \
    "$GALAXY_CARGO" build --manifest-path cpu-runtime/Cargo.toml \
      --release --locked --offline --bin simd_probe
}

build_variant "generic" "$GENERIC_TARGET" "$GENERIC_RUSTFLAGS"
build_variant "host-native" "$NATIVE_TARGET" "$NATIVE_RUSTFLAGS"

summarize_isa() {
  variant=$1
  binary=$2
  asm=$OUTPUT/${variant}-galaxy_hash_batch.asm
  evidence=$OUTPUT/${variant}-isa-evidence.txt

  if ! command -v objdump >/dev/null 2>&1; then
    printf '%s\n' "objdump not found; decoded ISA evidence skipped for $variant." > "$evidence"
    return
  fi

  if objdump -d -M intel --disassemble=galaxy_hash_batch "$binary" > "$asm" 2>/dev/null; then
    :
  elif objdump -d --disassemble=galaxy_hash_batch "$binary" > "$asm" 2>/dev/null; then
    :
  else
    printf '%s\n' "objdump could not disassemble galaxy_hash_batch for $variant." > "$evidence"
    rm -f "$asm"
    return
  fi

  mnemonics=$OUTPUT/${variant}-decoded-mnemonics.txt
  awk '
    /^[[:space:]]*[0-9A-Fa-f]+:/ {
      for (i = 2; i <= NF; i++) {
        if ($i ~ /^[A-Za-z][A-Za-z0-9_.]*$/) {
          print tolower($i)
          break
        }
      }
    }
  ' "$asm" > "$mnemonics"

  evex_lines=$(grep -Ec '^[[:space:]]*[0-9A-Fa-f]+:[[:space:]]+62([[:space:]]|$)' "$asm" || true)
  vex_lines=$(grep -Ec '^[[:space:]]*[0-9A-Fa-f]+:[[:space:]]+(c4|c5)([[:space:]]|$)' "$asm" || true)
  packed_count=$(grep -E '^(vp(add|sub|xor|mul|sr|sl|or|and)|p(add|sub|xor|mul|sr|sl|or|and))' "$mnemonics" | wc -l | tr -d ' ' || true)

  {
    printf 'variant=%s\n' "$variant"
    printf 'symbol=galaxy_hash_batch\n'
    printf 'evex_encoded_instruction_lines=%s\n' "$evex_lines"
    printf 'vex_encoded_instruction_lines=%s\n' "$vex_lines"
    printf 'decoded_packed_integer_vector_instruction_count=%s\n' "$packed_count"
    printf '%s\n' 'decoded_vector_mnemonics:'
    grep -E '^(v|p)(p(add|sub|xor|mul|sr|sl|or|and)|movd|movq)' "$mnemonics" \
      | sort | uniq -c || true
    printf '%s\n' 'interpretation:'
    printf '%s\n' '- EVEX-encoded instructions are direct evidence of EVEX/AVX-512-family code generation.'
    printf '%s\n' '- VEX prefix counts alone are not a vectorization claim; inspect the decoded packed-integer mnemonics above.'
    printf '%s\n' '- Register names alone are intentionally not used as ISA evidence.'
  } > "$evidence"
}

summarize_isa generic "$GENERIC_TARGET/release/simd_probe"
summarize_isa native "$NATIVE_TARGET/release/simd_probe"

run_sample() {
  variant=$1
  binary=$2
  pair=$3
  sequence=$4
  samples=$5
  receipt=$OUTPUT/${variant}-pair-${pair}.json

  printf 'pair=%s sequence=%s variant=%s\n' "$pair" "$sequence" "$variant"
  "$binary" --items "$ITEMS" --repeats 1 --receipt "$receipt" >/dev/null

  if ! grep -q '"full_parity_check": true' "$receipt"; then
    printf 'parity gate missing or false in %s\n' "$receipt" >&2
    exit 1
  fi
  ns=$(sed -n 's/.*"median_ns": \([0-9][0-9]*\).*/\1/p' "$receipt")
  checksum=$(sed -n 's/.*"checksum": "\([0-9a-fA-F][0-9a-fA-F]*\)".*/\1/p' "$receipt")
  if [ -z "$ns" ] || [ -z "$checksum" ]; then
    printf 'failed to parse timing/checksum from %s\n' "$receipt" >&2
    exit 1
  fi
  printf '%s\t%s\t%s\n' "$pair" "$ns" "$checksum" >> "$samples"
  printf '%s\t%s\t%s\n' "$pair" "$sequence" "$variant" >> "$ORDER_LOG"
}

printf '\n%s\n' "== Interleaved paired execution samples =="
pair=1
while [ "$pair" -le "$PAIRS" ]; do
  if [ $((pair % 2)) -eq 1 ]; then
    run_sample generic "$GENERIC_TARGET/release/simd_probe" "$pair" 1 "$GENERIC_SAMPLES"
    run_sample native "$NATIVE_TARGET/release/simd_probe" "$pair" 2 "$NATIVE_SAMPLES"
  else
    run_sample native "$NATIVE_TARGET/release/simd_probe" "$pair" 1 "$NATIVE_SAMPLES"
    run_sample generic "$GENERIC_TARGET/release/simd_probe" "$pair" 2 "$GENERIC_SAMPLES"
  fi
  pair=$((pair + 1))
done

median_from_samples() {
  cut -f2 "$1" | sort -n | awk '
    { values[NR] = $1 }
    END {
      if (NR == 0) exit 1
      if (NR % 2 == 1) print values[(NR + 1) / 2]
      else print int((values[NR / 2] + values[NR / 2 + 1]) / 2)
    }
  '
}

unique_checksum() {
  values=$(cut -f3 "$1" | sort -u)
  count=$(printf '%s\n' "$values" | sed '/^$/d' | wc -l | tr -d ' ')
  if [ "$count" -ne 1 ]; then
    printf 'checksum changed across paired samples in %s\n' "$1" >&2
    exit 1
  fi
  printf '%s\n' "$values"
}

GENERIC_MEDIAN=$(median_from_samples "$GENERIC_SAMPLES")
NATIVE_MEDIAN=$(median_from_samples "$NATIVE_SAMPLES")
GENERIC_CHECKSUM=$(unique_checksum "$GENERIC_SAMPLES")
NATIVE_CHECKSUM=$(unique_checksum "$NATIVE_SAMPLES")
if [ "$GENERIC_CHECKSUM" != "$NATIVE_CHECKSUM" ]; then
  printf 'generic/native checksum mismatch: %s != %s\n' "$GENERIC_CHECKSUM" "$NATIVE_CHECKSUM" >&2
  exit 1
fi
SPEEDUP=$(awk -v generic="$GENERIC_MEDIAN" -v native="$NATIVE_MEDIAN" 'BEGIN { if (native == 0) exit 1; printf "%.9f", generic / native }')

cat > "$GENERIC_SUMMARY" <<EOF
{
  "schema": "galaxy.cpu-simd-series.v1",
  "variant": "generic",
  "target_cpu": "$GENERIC_CPU",
  "items_per_sample": $ITEMS,
  "paired_samples": $PAIRS,
  "median_ns": $GENERIC_MEDIAN,
  "checksum": "$GENERIC_CHECKSUM",
  "full_parity_check_each_sample": true,
  "timing_order": "alternating-paired"
}
EOF

cat > "$NATIVE_SUMMARY" <<EOF
{
  "schema": "galaxy.cpu-simd-series.v1",
  "variant": "native",
  "target_cpu": "native",
  "items_per_sample": $ITEMS,
  "paired_samples": $PAIRS,
  "median_ns": $NATIVE_MEDIAN,
  "checksum": "$NATIVE_CHECKSUM",
  "full_parity_check_each_sample": true,
  "timing_order": "alternating-paired"
}
EOF

{
  printf 'generic_median_ns=%s\n' "$GENERIC_MEDIAN"
  printf 'native_median_ns=%s\n' "$NATIVE_MEDIAN"
  printf 'native_speedup_vs_generic=%s\n' "$SPEEDUP"
  printf 'checksum=%s\n' "$GENERIC_CHECKSUM"
  printf 'paired_samples=%s\n' "$PAIRS"
} > "$COMPARISON"

printf '\n%s\n' "GALAXY SIMD probe completed."
printf 'output=%s\n' "$OUTPUT"
printf 'generic_summary=%s\n' "$GENERIC_SUMMARY"
printf 'native_summary=%s\n' "$NATIVE_SUMMARY"
printf 'comparison=%s\n' "$COMPARISON"
printf 'host_evidence=%s\n' "$OUTPUT/host.txt"
printf 'generic_isa_evidence=%s\n' "$OUTPUT/generic-isa-evidence.txt"
printf 'native_isa_evidence=%s\n' "$OUTPUT/native-isa-evidence.txt"
