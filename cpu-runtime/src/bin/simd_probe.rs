// SPDX-License-Identifier: Apache-2.0
//! Deterministic SIMD/autovectorization probe for GALAXY's contribution hash.
//!
//! This binary does not change the canonical CPU runtime. It isolates the
//! structure-of-arrays contribution stage so generic and host-native builds can
//! be compared without changing GALAXY's checksum contract.

use galaxy_retro_math::hash32;
use std::{
    env, fs,
    hint::black_box,
    path::{Path, PathBuf},
    time::Instant,
};

const DEFAULT_ITEMS: usize = 4_194_304;
const DEFAULT_REPEATS: usize = 7;
const MAX_ITEMS: usize = 16_777_216;
const MAX_REPEATS: usize = 25;

#[derive(Debug)]
struct Config {
    items: usize,
    repeats: usize,
    receipt: Option<PathBuf>,
}

fn usage() -> &'static str {
    "Usage:\n  simd_probe [--items N] [--repeats N] [--receipt PATH]\n\nDefaults:\n  items=4194304 repeats=7\n"
}

fn parse_positive(flag: &str, value: String, maximum: usize) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{flag} must be a positive decimal integer"))?;
    if parsed == 0 || parsed > maximum {
        return Err(format!("{flag} must be in 1..={maximum}"));
    }
    Ok(parsed)
}

fn parse_config(args: &[String]) -> Result<Config, String> {
    let mut config = Config {
        items: DEFAULT_ITEMS,
        repeats: DEFAULT_REPEATS,
        receipt: None,
    };
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        if flag == "--help" || flag == "-h" {
            return Err(usage().to_string());
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("{flag} requires a value"))?
            .clone();
        match flag {
            "--items" => config.items = parse_positive(flag, value, MAX_ITEMS)?,
            "--repeats" => config.repeats = parse_positive(flag, value, MAX_REPEATS)?,
            "--receipt" => config.receipt = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown option: {flag}\n\n{}", usage())),
        }
        index += 2;
    }
    Ok(config)
}

#[inline(always)]
fn hash32_local(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

#[inline(always)]
fn contribution_reference(id_lo: u32, id_hi: u32, x: u32, y: u32) -> u64 {
    let first = hash32(x ^ hash32(y) ^ id_lo ^ hash32(id_hi ^ 0xa511_e9b3));
    let second = hash32(first ^ id_hi ^ 0x85eb_ca6b);
    ((first as u64) << 32) | second as u64
}

/// SoA form of GALAXY's exact contribution hash.
///
/// The symbol is intentionally left unmangled so `objdump --disassemble` can
/// inspect the decoded instructions selected by LLVM. The function contains no
/// ISA-specific intrinsics; source portability is preserved while target CPU
/// selection remains a build-time concern.
#[no_mangle]
#[inline(never)]
pub fn galaxy_hash_batch(
    id_lo: &[u32],
    id_hi: &[u32],
    x: &[u32],
    y: &[u32],
    out: &mut [u64],
) {
    debug_assert_eq!(id_lo.len(), id_hi.len());
    debug_assert_eq!(id_lo.len(), x.len());
    debug_assert_eq!(id_lo.len(), y.len());
    debug_assert_eq!(id_lo.len(), out.len());

    for index in 0..out.len() {
        let first = hash32_local(
            x[index]
                ^ hash32_local(y[index])
                ^ id_lo[index]
                ^ hash32_local(id_hi[index] ^ 0xa511_e9b3),
        );
        let second = hash32_local(first ^ id_hi[index] ^ 0x85eb_ca6b);
        out[index] = ((first as u64) << 32) | second as u64;
    }
}

fn build_inputs(items: usize) -> (Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>) {
    let mut id_lo = Vec::with_capacity(items);
    let mut id_hi = Vec::with_capacity(items);
    let mut x = Vec::with_capacity(items);
    let mut y = Vec::with_capacity(items);

    for index in 0..items {
        let lane = index as u32;
        let lo = hash32(lane.wrapping_mul(0x9e37_79b9) ^ 0x243f_6a88);
        let hi = hash32(lane.wrapping_mul(0x85eb_ca6b) ^ 0x1319_8a2e);
        id_lo.push(lo);
        id_hi.push(hi);
        x.push(hash32(lo ^ 0xa409_3822));
        y.push(hash32(hi ^ 0x299f_31d0));
    }

    (id_lo, id_hi, x, y)
}

fn checksum(values: &[u64]) -> u64 {
    values
        .iter()
        .fold(0_u64, |acc, value| acc.wrapping_add(*value))
}

fn median_sorted(values: &mut [u128]) -> u128 {
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        let lower = values[middle - 1];
        let upper = values[middle];
        lower + (upper - lower) / 2
    } else {
        values[middle]
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn runtime_features() -> (bool, bool, bool) {
    (
        std::arch::is_x86_feature_detected!("avx2"),
        std::arch::is_x86_feature_detected!("fma"),
        std::arch::is_x86_feature_detected!("avx512f"),
    )
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn runtime_features() -> (bool, bool, bool) {
    (false, false, false)
}

fn write_receipt(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
    }
    fs::write(path, content).map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{}", usage());
        return Ok(());
    }
    let config = parse_config(&args)?;
    let (runtime_avx2, runtime_fma, runtime_avx512f) = runtime_features();

    let (id_lo, id_hi, x, y) = build_inputs(config.items);
    let mut out = vec![0_u64; config.items];

    galaxy_hash_batch(&id_lo, &id_hi, &x, &y, &mut out);
    for index in 0..config.items {
        let expected = contribution_reference(id_lo[index], id_hi[index], x[index], y[index]);
        if out[index] != expected {
            return Err(format!(
                "SIMD probe parity failure at index {index}: {:016x} != {:016x}",
                out[index], expected
            ));
        }
    }
    let expected_checksum = checksum(&out);

    let mut timings = Vec::with_capacity(config.repeats);
    for _ in 0..config.repeats {
        let started = Instant::now();
        galaxy_hash_batch(&id_lo, &id_hi, &x, &y, &mut out);
        let elapsed = started.elapsed().as_nanos();
        black_box(out[config.items / 2]);
        if checksum(&out) != expected_checksum {
            return Err("SIMD probe checksum changed between repeats".into());
        }
        timings.push(elapsed);
    }

    let best_ns = *timings.iter().min().expect("positive repeat count");
    let median_ns = median_sorted(&mut timings);
    let ns_per_item = median_ns as f64 / config.items as f64;

    // `cfg!(target_feature = ...)` is useful for stable target features such as
    // AVX2/FMA, but it is not a trustworthy AVX-512 code-generation receipt on
    // the crate's Rust 1.85 MSRV. AVX-512 codegen evidence is therefore kept
    // outside this receipt and derived from decoded disassembly by the runner.
    let receipt = format!(
        "{{\n  \"schema\": \"galaxy.cpu-simd-probe.v2\",\n  \"architecture\": \"{}\",\n  \"os\": \"{}\",\n  \"items\": {},\n  \"repeats\": {},\n  \"runtime_avx2\": {},\n  \"runtime_fma\": {},\n  \"runtime_avx512f\": {},\n  \"cfg_target_feature_avx2\": {},\n  \"cfg_target_feature_fma\": {},\n  \"avx512_codegen_evidence\": \"external-decoded-disassembly-required\",\n  \"best_ns\": {},\n  \"median_ns\": {},\n  \"median_ns_per_item\": {:.9},\n  \"checksum\": \"{:016x}\",\n  \"full_parity_check\": true,\n  \"claim_boundary\": \"Isolated structure-of-arrays contribution-hash evidence only; AVX-512 code generation is established from decoded disassembly, not Rust cfg target_feature; this is not an end-to-end GALAXY runtime speedup claim.\"\n}}\n",
        env::consts::ARCH,
        env::consts::OS,
        config.items,
        config.repeats,
        runtime_avx2,
        runtime_fma,
        runtime_avx512f,
        cfg!(target_feature = "avx2"),
        cfg!(target_feature = "fma"),
        best_ns,
        median_ns,
        ns_per_item,
        expected_checksum,
    );

    print!("{receipt}");
    if let Some(path) = &config.receipt {
        write_receipt(path, &receipt)?;
        eprintln!("receipt={}", path.display());
    }
    Ok(())
}
