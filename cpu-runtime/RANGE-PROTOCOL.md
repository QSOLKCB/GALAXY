# GALAXY CPU range protocol v1

`galaxy-cpu range` regenerates a bounded interval of resident samples using
GALAXY's own addressing, physics, projection, and reduction. It accepts a
half-open interval of **resident sample indices**, not raw u64 logical IDs:

```sh
cargo run --manifest-path cpu-runtime/Cargo.toml --release --bin galaxy-cpu -- \
  range --logical 18446744073709551615 --resident 8388608 \
  --start 0 --end 4194304 --frames 8 --seed 303 --backend lut
```

The single stdout line is tab-separated, ordered, and versioned:

```text
galaxy.cpu-range.v1\tlogical=...\tresident=...\tstart=...\tend=...\tframes=...\tseed=...\tbackend=bam-lut-q30\tchecksum=0123456789abcdef
```

All required arguments are exact decimal integers. The backend is `lut` or
`float`; the response names the actual projection. Intervals must be nonempty,
inside the resident population, and no larger than 16,777,216 samples. A
caller combines verified, gap-free partials with wrapping u64 addition. The
global ID for sample index `i` remains `floor(i * logical / resident)`, using
the **full** resident count even when only a subset is generated.

The range command is a CPU entrypoint. It is not accelerator evidence. The
canonical `bench` and `verify` paths and archived checksums are unchanged.
