#!/usr/bin/env bash
set -euo pipefail

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
MANIFEST="$ROOT/cpu-runtime/Cargo.toml"
OUT=${GALAXY_MEMORY_OUT:-"$ROOT/runs/cpu-memory-wall-$(date -u +%Y%m%dT%H%M%SZ)"}
RESIDENT=${GALAXY_MEMORY_RESIDENT:-1048576}
FRAMES=${GALAXY_MEMORY_FRAMES:-8}
REPEATS=${GALAXY_MEMORY_REPEATS:-5}
WORKERS=${GALAXY_MEMORY_WORKERS:-$(getconf _NPROCESSORS_ONLN 2>/dev/null || printf '1')}
SEED=${GALAXY_MEMORY_SEED:-303}
TILES=${GALAXY_MEMORY_TILES:-"1024 512 256 128 64"}
REDUCTIONS=${GALAXY_MEMORY_REDUCTIONS:-"materialized fused"}
LUTS=${GALAXY_MEMORY_LUTS:-"full cosine quarter"}

if [ -e "$OUT" ] && [ "$(find "$OUT" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null || true)" ]; then
  printf 'Refusing to mix evidence in non-empty directory: %s\n' "$OUT" >&2
  exit 2
fi
mkdir -p "$OUT/receipts" "$OUT/logs" "$OUT/target-native"

{
  printf 'timestamp_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  uname -a || true
  grep -m1 '^model name' /proc/cpuinfo 2>/dev/null || true
  printf 'resident=%s\nframes=%s\nrepeats=%s\nworkers=%s\nseed=%s\n' \
    "$RESIDENT" "$FRAMES" "$REPEATS" "$WORKERS" "$SEED"
  printf 'tiles=%s\nreductions=%s\nluts=%s\n' "$TILES" "$REDUCTIONS" "$LUTS"
  cargo --version || true
  rustc --version || true
} > "$OUT/host.txt"

export CARGO_TARGET_DIR="$OUT/target-native"
if [ -n "${RUSTFLAGS:-}" ]; then
  export RUSTFLAGS="$RUSTFLAGS -C target-cpu=native"
else
  export RUSTFLAGS="-C target-cpu=native"
fi

cargo build --manifest-path "$MANIFEST" --release --locked --offline --bin memory_wall_probe
BIN="$CARGO_TARGET_DIR/release/memory_wall_probe"
[ -x "$BIN" ] || { printf 'Missing probe binary: %s\n' "$BIN" >&2; exit 2; }

"$BIN" verify --workers "$WORKERS" | tee "$OUT/verify.txt"

printf 'reduction\tlut\ttile\tmedian_ns\tbest_ns\tchecksum\tpeak_rss_kib\tlut_storage_bytes\tworking_bytes_per_tiled_particle\tworker_microtile_capacity_bytes\talgorithmic_working_set_bytes\treceipt\n' > "$OUT/matrix.tsv"

for reduction in $REDUCTIONS; do
  for lut in $LUTS; do
    for tile in $TILES; do
      stem="${reduction}-${lut}-tile${tile}"
      receipt="$OUT/receipts/$stem.json"
      log="$OUT/logs/$stem.txt"
      "$BIN" bench \
        --reduce "$reduction" \
        --lut "$lut" \
        --logical 18446744073709551615 \
        --resident "$RESIDENT" \
        --frames "$FRAMES" \
        --workers "$WORKERS" \
        --tile "$tile" \
        --repeats "$REPEATS" \
        --seed "$SEED" \
        --receipt "$receipt" | tee "$log"
      python3 - "$receipt" >> "$OUT/matrix.tsv" <<'PY'
import json, sys
p=sys.argv[1]
with open(p, encoding='utf-8') as f:
    d=json.load(f)
print("\t".join(str(x) for x in [
    d['reduction_mode'], d['lut_mode'], d['microtile_particles'],
    d['median_ns'], d['best_ns'], d['checksum'], d['peak_rss_kib'],
    d['lut_storage_bytes'], d['working_bytes_per_tiled_particle'],
    d['worker_microtile_capacity_bytes'], d['algorithmic_working_set_bytes'], p
]))
PY
    done
  done
done

python3 - "$OUT/matrix.tsv" > "$OUT/summary.txt" <<'PY'
import csv, sys
path=sys.argv[1]
with open(path, newline='', encoding='utf-8') as f:
    rows=list(csv.DictReader(f, delimiter='\t'))
checksums={r['checksum'] for r in rows}
if len(checksums) != 1:
    raise SystemExit(f"checksum mismatch across matrix: {sorted(checksums)}")
for r in rows:
    for key in ('median_ns','best_ns','lut_storage_bytes','working_bytes_per_tiled_particle','worker_microtile_capacity_bytes','algorithmic_working_set_bytes'):
        r[key]=int(r[key])
print(f"cases={len(rows)}")
print(f"checksum={next(iter(checksums))}")
best_time=min(rows, key=lambda r:r['median_ns'])
best_mem=min(rows, key=lambda r:r['algorithmic_working_set_bytes'])
print("fastest=" + ",".join(f"{k}={best_time[k]}" for k in ('reduction','lut','tile','median_ns','algorithmic_working_set_bytes')))
print("smallest_algorithmic_working_set=" + ",".join(f"{k}={best_mem[k]}" for k in ('reduction','lut','tile','median_ns','algorithmic_working_set_bytes')))
base=next((r for r in rows if r['reduction']=='materialized-contribution-array' and r['lut']=='full-sine-cosine-16k' and r['tile']==1024), None)
if base:
    print(f"baseline_algorithmic_working_set_bytes={base['algorithmic_working_set_bytes']}")
    print(f"baseline_median_ns={base['median_ns']}")
    print(f"smallest_vs_baseline_memory_ratio={base['algorithmic_working_set_bytes']/best_mem['algorithmic_working_set_bytes']:.9f}")
PY

if command -v objdump >/dev/null 2>&1; then
  objdump -d --disassemble=galaxy_memory_hash_batch "$BIN" > "$OUT/hash-batch.asm" || true
  objdump -d --disassemble=galaxy_memory_hash_reduce "$BIN" > "$OUT/hash-reduce.asm" || true
fi

find "$OUT" -type f -not -path '*/target-native/*' -not -name 'SHA256SUMS.txt' -print0 \
  | sort -z \
  | xargs -0 sha256sum > "$OUT/SHA256SUMS.txt"

cat "$OUT/summary.txt"
printf 'evidence_dir=%s\n' "$OUT"
