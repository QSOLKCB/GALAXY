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

GALAXY_RUSTC=${GALAXY_RUSTC:-rustc}
if ! command -v "$GALAXY_RUSTC" >/dev/null 2>&1; then
  GALAXY_RUSTC=${RUSTC:-rustc}
fi
if ! command -v "$GALAXY_RUSTC" >/dev/null 2>&1; then
  printf '%s\n' "GALAXY SIMD probe: rustc is required to resolve the host target." >&2
  exit 1
fi

normalize_positive_decimal() {
  name=$1
  raw=$2
  maximum=$3
  normalized=$(awk -v value="$raw" -v maximum="$maximum" '
    BEGIN {
      if (value !~ /^\+?[0-9]+$/) exit 1
      sub(/^\+/, "", value)
      sub(/^0+/, "", value)
      if (value == "") value = "0"
      number = value + 0
      if (number < 1 || number > maximum || number != int(number)) exit 1
      printf "%.0f\n", number
    }
  ') || {
    printf '%s must be a decimal integer in 1..=%s.\n' "$name" "$maximum" >&2
    exit 2
  }
  printf '%s\n' "$normalized"
}

ITEMS=$(normalize_positive_decimal GALAXY_SIMD_ITEMS "${GALAXY_SIMD_ITEMS:-4194304}" 16777216)
PAIRS=$(normalize_positive_decimal GALAXY_SIMD_REPEATS "${GALAXY_SIMD_REPEATS:-7}" 25)

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
HOST_TARGET=$("$GALAXY_RUSTC" -vV | sed -n 's/^host: //p')
if [ -z "$HOST_TARGET" ]; then
  printf '%s\n' "GALAXY SIMD probe: could not resolve rustc host target." >&2
  exit 1
fi
case "$HOST_ARCH" in
  x86_64|amd64) GENERIC_CPU=x86-64 ;;
  *) GENERIC_CPU=generic ;;
esac
case "$HOST_TARGET" in
  *-windows-*) EXE_SUFFIX=.exe ;;
  *) EXE_SUFFIX= ;;
esac
GENERIC_RUSTFLAGS="-C target-cpu=$GENERIC_CPU"
NATIVE_RUSTFLAGS='-C target-cpu=native'
GENERIC_BINARY=$GENERIC_TARGET/$HOST_TARGET/release/simd_probe$EXE_SUFFIX
NATIVE_BINARY=$NATIVE_TARGET/$HOST_TARGET/release/simd_probe$EXE_SUFFIX

mkdir -p "$OUTPUT"
: > "$GENERIC_SAMPLES"
: > "$NATIVE_SAMPLES"
printf 'pair\tsequence\tvariant\n' > "$ORDER_LOG"

{
  printf 'date_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf 'uname=%s\n' "$(uname -a)"
  printf 'host_arch=%s\n' "$HOST_ARCH"
  printf 'host_target=%s\n' "$HOST_TARGET"
  printf 'generic_rustflags=%s\n' "$GENERIC_RUSTFLAGS"
  printf 'native_rustflags=%s\n' "$NATIVE_RUSTFLAGS"
  "$GALAXY_CARGO" --version || true
  "$GALAXY_RUSTC" --version -v || true
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
  # CLI --target pins Cargo to the selected rustc's host triple. Explicit RUSTC
  # aligns compiler provenance with the compiler Cargo actually invokes. This
  # probe also requires its exported inspection symbol to survive release
  # linking, so override caller/config stripping for this evidence-only build.
  env -u CARGO_BUILD_TARGET -u CARGO_ENCODED_RUSTFLAGS \
    CARGO_TARGET_DIR="$target_dir" \
    CARGO_PROFILE_RELEASE_STRIP=false \
    RUSTC="$GALAXY_RUSTC" \
    RUSTFLAGS="$rustflags" \
    "$GALAXY_CARGO" build --manifest-path cpu-runtime/Cargo.toml \
      --release --locked --offline --target "$HOST_TARGET" --bin simd_probe
}

build_variant "generic" "$GENERIC_TARGET" "$GENERIC_RUSTFLAGS"
build_variant "host-native" "$NATIVE_TARGET" "$NATIVE_RUSTFLAGS"

for binary in "$GENERIC_BINARY" "$NATIVE_BINARY"; do
  if [ ! -x "$binary" ]; then
    printf 'expected SIMD probe artifact not found or not executable: %s\n' "$binary" >&2
    exit 1
  fi
done

summarize_isa() {
  variant=$1
  binary=$2
  asm=$OUTPUT/${variant}-galaxy_hash_batch.asm
  decoded_asm=$OUTPUT/${variant}-galaxy_hash_batch.decoded.asm
  evidence=$OUTPUT/${variant}-isa-evidence.txt
  mnemonics=$OUTPUT/${variant}-decoded-mnemonics.txt

  if ! command -v objdump >/dev/null 2>&1; then
    {
      printf 'variant=%s\n' "$variant"
      printf 'symbol=galaxy_hash_batch\n'
      printf 'evidence_available=false\n'
      printf 'reason=objdump-not-found\n'
    } > "$evidence"
    return
  fi

  if objdump -d -M intel --disassemble=galaxy_hash_batch "$binary" > "$asm" 2>/dev/null; then
    :
  elif objdump -d --disassemble=galaxy_hash_batch "$binary" > "$asm" 2>/dev/null; then
    :
  else
    {
      printf 'variant=%s\n' "$variant"
      printf 'symbol=galaxy_hash_batch\n'
      printf 'evidence_available=false\n'
      printf 'reason=objdump-disassembly-failed\n'
    } > "$evidence"
    rm -f "$asm"
    return
  fi

  if objdump -d -M intel --no-show-raw-insn --disassemble=galaxy_hash_batch "$binary" > "$decoded_asm" 2>/dev/null \
    || objdump -d --no-show-raw-insn --disassemble=galaxy_hash_batch "$binary" > "$decoded_asm" 2>/dev/null; then
    awk '
      /^[[:space:]]*[0-9A-Fa-f]+:/ {
        line = $0
        sub(/^[[:space:]]*[0-9A-Fa-f]+:[[:space:]]*/, "", line)
        split(line, fields, /[[:space:]]+/)
        if (fields[1] ~ /^[A-Za-z][A-Za-z0-9_.]*$/) print tolower(fields[1])
      }
    ' "$decoded_asm" > "$mnemonics"
  else
    rm -f "$decoded_asm"
    awk '
      /^[[:space:]]*[0-9A-Fa-f]+:/ {
        line = $0
        sub(/^[[:space:]]*[0-9A-Fa-f]+:[[:space:]]*/, "", line)
        count = split(line, fields, /[[:space:]]+/)
        for (i = 1; i <= count; i++) {
          if (fields[i] ~ /^[0-9A-Fa-f][0-9A-Fa-f]$/) continue
          if (fields[i] ~ /^[A-Za-z][A-Za-z0-9_.]*$/) {
            print tolower(fields[i])
            break
          }
        }
      }
    ' "$asm" > "$mnemonics"
  fi

  # GNU objdump may return status 0 for --disassemble=<symbol> even when the
  # symbol is absent (for example after release stripping), emitting only
  # headers. Never turn that into a misleading zero-vectorization result.
  if [ ! -s "$mnemonics" ]; then
    {
      printf 'variant=%s\n' "$variant"
      printf 'symbol=galaxy_hash_batch\n'
      printf 'evidence_available=false\n'
      printf 'reason=probe-symbol-body-not-decoded\n'
    } > "$evidence"
    rm -f "$decoded_asm" "$mnemonics"
    return
  fi

  evex_mnemonics=$OUTPUT/${variant}-evex-mnemonics.txt
  awk '
    /^[[:space:]]*[0-9A-Fa-f]+:[[:space:]]+62([[:space:]]|$)/ {
      line = $0
      sub(/^[[:space:]]*[0-9A-Fa-f]+:[[:space:]]*/, "", line)
      count = split(line, fields, /[[:space:]]+/)
      for (i = 1; i <= count; i++) {
        if (fields[i] ~ /^[0-9A-Fa-f][0-9A-Fa-f]$/) continue
        if (fields[i] ~ /^[A-Za-z][A-Za-z0-9_.]*$/) {
          print tolower(fields[i])
          break
        }
      }
    }
  ' "$asm" > "$evex_mnemonics"

  vex_mnemonics=$OUTPUT/${variant}-vex-mnemonics.txt
  awk '
    /^[[:space:]]*[0-9A-Fa-f]+:[[:space:]]+(c4|c5)([[:space:]]|$)/ {
      line = $0
      sub(/^[[:space:]]*[0-9A-Fa-f]+:[[:space:]]*/, "", line)
      count = split(line, fields, /[[:space:]]+/)
      for (i = 1; i <= count; i++) {
        if (fields[i] ~ /^[0-9A-Fa-f][0-9A-Fa-f]$/) continue
        if (fields[i] ~ /^[A-Za-z][A-Za-z0-9_.]*$/) {
          print tolower(fields[i])
          break
        }
      }
    }
  ' "$asm" > "$vex_mnemonics"

  evex_lines=$(sed '/^$/d' "$evex_mnemonics" | wc -l | tr -d ' ')
  vex_lines=$(sed '/^$/d' "$vex_mnemonics" | wc -l | tr -d ' ')
  packed_mnemonic_re='^(vp(add|sub|xor|mul|sr|sl|or|and)|p(add|sub|xor|mul|sr|sl|or|and))'
  packed_count=$(grep -E "$packed_mnemonic_re" "$mnemonics" | wc -l | tr -d ' ' || true)
  decoded_evex_count=$evex_lines

  {
    printf 'variant=%s\n' "$variant"
    printf 'symbol=galaxy_hash_batch\n'
    printf 'evidence_available=true\n'
    printf 'evex_encoded_instruction_lines=%s\n' "$evex_lines"
    printf 'decoded_evex_mnemonic_count=%s\n' "$decoded_evex_count"
    printf 'vex_encoded_instruction_lines=%s\n' "$vex_lines"
    printf 'decoded_packed_integer_vector_instruction_count=%s\n' "$packed_count"
    printf '%s\n' 'decoded_packed_integer_vector_mnemonics:'
    grep -E "$packed_mnemonic_re" "$mnemonics" | sort | uniq -c || true
    printf '%s\n' 'decoded_evex_mnemonics:'
    sort "$evex_mnemonics" | uniq -c || true
    printf '%s\n' 'interpretation:'
    printf '%s\n' '- EVEX counts include only prefix-matched rows with decoded mnemonics; wrapped byte continuations are excluded.'
    printf '%s\n' '- Packed-integer counts include legacy p... and VEX/EVEX vp... arithmetic/shift families.'
    printf '%s\n' '- Missing symbol/body evidence is reported as unavailable, never as zero vectorization.'
    printf '%s\n' '- Register names alone are intentionally not used as ISA evidence.'
  } > "$evidence"
}

summarize_isa generic "$GENERIC_BINARY"
summarize_isa native "$NATIVE_BINARY"

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
    run_sample generic "$GENERIC_BINARY" "$pair" 1 "$GENERIC_SAMPLES"
    run_sample native "$NATIVE_BINARY" "$pair" 2 "$NATIVE_SAMPLES"
  else
    run_sample native "$NATIVE_BINARY" "$pair" 1 "$NATIVE_SAMPLES"
    run_sample generic "$GENERIC_BINARY" "$pair" 2 "$GENERIC_SAMPLES"
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
  printf 'host_target=%s\n' "$HOST_TARGET"
} > "$COMPARISON"

printf '\n%s\n' "GALAXY SIMD probe completed."
printf 'output=%s\n' "$OUTPUT"
printf 'generic_summary=%s\n' "$GENERIC_SUMMARY"
printf 'native_summary=%s\n' "$NATIVE_SUMMARY"
printf 'comparison=%s\n' "$COMPARISON"
printf 'host_evidence=%s\n' "$OUTPUT/host.txt"
printf 'generic_isa_evidence=%s\n' "$OUTPUT/generic-isa-evidence.txt"
printf 'native_isa_evidence=%s\n' "$OUTPUT/native-isa-evidence.txt"
