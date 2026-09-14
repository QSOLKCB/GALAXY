// SPDX-License-Identifier: Apache-2.0
//! Headless deterministic native CPU runtime for GALAXY.
//!
//! The runtime keeps the logical population as an exact u64 address space and
//! materialises only a bounded resident set. Projection is available through a
//! native floating-point baseline and the PR #7 BAM32 + interpolated LUT path.
//! Parallel execution uses only the Rust standard library and deterministic
//! contiguous chunks.

use galaxy_retro_math::{hash32, sin_cos_q30};
use galaxy_sampler::physics::{self, Parameters};
use std::{
    env, fs,
    hint::black_box,
    path::{Path, PathBuf},
    thread,
    time::Instant,
};

const ADDRESSING: &str = "split-u64-hash32-avalanche-v1";
const RECEIPT_SCHEMA: &str = "galaxy.cpu-runtime-receipt.v1";
const MAX_WORKERS: usize = 256;
const MAX_RESIDENT: usize = 16_777_216;
const MAX_FRAMES: usize = 100_000;
const MAX_REPEATS: usize = 25;
const DEFAULT_RESIDENT: usize = 1_048_576;
const DEFAULT_FRAMES: usize = 8;
const DEFAULT_REPEATS: usize = 5;
const DEFAULT_SEED: u32 = 303;
const BULGE: f64 = 0.18;
const SHEAR: f64 = 0.35;
const PHASE_STEP: f64 = 1.0 / 60.0;
const TURN_SCALE: f64 = std::f64::consts::TAU / 4_294_967_296.0;

const LUT_BITS: u32 = 14;
const LUT_SIZE: usize = 1 << LUT_BITS;
const LUT_SHIFT: u32 = 32 - LUT_BITS;
const LUT_FRAC_MASK: u32 = (1_u32 << LUT_SHIFT) - 1;

#[derive(Clone, Copy, Debug)]
struct Particle {
    id: u64,
    radius_q16: i32,
    initial_bam: u32,
    delta_bam: u32,
    initial_rad: f64,
    delta_rad: f64,
}

#[derive(Clone, Copy, Debug)]
enum Backend {
    Float,
    Lut,
}

impl Backend {
    fn name(self) -> &'static str {
        match self {
            Self::Float => "float-libm",
            Self::Lut => "bam-lut-q30",
        }
    }
}

#[derive(Debug)]
struct Lut {
    cos: Vec<i32>,
    sin: Vec<i32>,
}

impl Lut {
    fn build() -> Self {
        let mut cos = Vec::with_capacity(LUT_SIZE);
        let mut sin = Vec::with_capacity(LUT_SIZE);
        for index in 0..LUT_SIZE {
            let angle = (index as u32) << LUT_SHIFT;
            let (c, s) = sin_cos_q30(angle);
            cos.push(c as i32);
            sin.push(s as i32);
        }
        Self { cos, sin }
    }

    fn sin_cos(&self, angle: u32) -> (i64, i64) {
        let index = (angle >> LUT_SHIFT) as usize;
        let next = (index + 1) & (LUT_SIZE - 1);
        let fraction = (angle & LUT_FRAC_MASK) as i64;
        let c0 = self.cos[index] as i64;
        let s0 = self.sin[index] as i64;
        let c = c0 + (((self.cos[next] as i64 - c0) * fraction) >> LUT_SHIFT);
        let s = s0 + (((self.sin[next] as i64 - s0) * fraction) >> LUT_SHIFT);
        (c, s)
    }
}

#[derive(Clone, Copy, Debug)]
struct Timing {
    best_ns: u128,
    median_ns: u128,
    checksum: u64,
}

#[derive(Clone, Copy, Debug)]
struct BackendEvidence {
    scalar: Timing,
    parallel: Timing,
    speedup: f64,
    checksum_match: bool,
    multicore_claim: bool,
}

#[derive(Clone, Debug)]
struct Config {
    logical: u64,
    resident: usize,
    frames: usize,
    requested_workers: usize,
    repeats: usize,
    seed: u32,
    receipt: Option<PathBuf>,
}

fn usage() -> &'static str {
    "Usage:\n  galaxy-cpu verify [--workers N]\n  galaxy-cpu bench [--logical U64] [--resident N] [--frames N] [--workers N] [--repeats N] [--seed U32] [--receipt PATH]\n\nDefaults:\n  logical=18446744073709551615 resident=1048576 frames=8 repeats=5 seed=303\n  workers=std::thread::available_parallelism()\n"
}

fn help_requested(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--help" || arg == "-h")
}

fn parse_usize(flag: &str, value: String, maximum: usize) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{flag} must be a positive decimal integer"))?;
    if parsed == 0 || parsed > maximum {
        return Err(format!("{flag} must be in 1..={maximum}"));
    }
    Ok(parsed)
}

fn parse_config(args: &[String]) -> Result<Config, String> {
    let available = thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .clamp(1, MAX_WORKERS);
    let mut config = Config {
        logical: u64::MAX,
        resident: DEFAULT_RESIDENT,
        frames: DEFAULT_FRAMES,
        requested_workers: available,
        repeats: DEFAULT_REPEATS,
        seed: DEFAULT_SEED,
        receipt: None,
    };

    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("{flag} requires a value"))?
            .clone();
        match flag {
            "--logical" => {
                config.logical = value
                    .parse::<u64>()
                    .map_err(|_| "--logical must be an exact decimal u64".to_string())?;
                if config.logical == 0 {
                    return Err("--logical must be greater than zero".into());
                }
            }
            "--resident" => config.resident = parse_usize(flag, value, MAX_RESIDENT)?,
            "--frames" => config.frames = parse_usize(flag, value, MAX_FRAMES)?,
            "--workers" => config.requested_workers = parse_usize(flag, value, MAX_WORKERS)?,
            "--repeats" => config.repeats = parse_usize(flag, value, MAX_REPEATS)?,
            "--seed" => {
                config.seed = value
                    .parse::<u32>()
                    .map_err(|_| "--seed must fit u32".to_string())?;
            }
            "--receipt" => config.receipt = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown option: {flag}")),
        }
        index += 2;
    }

    if config.resident as u64 > config.logical {
        return Err("resident particles must not exceed the logical population".into());
    }
    Ok(config)
}

fn available_parallelism() -> usize {
    thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .clamp(1, MAX_WORKERS)
}

fn effective_workers(work_items: usize, requested_workers: usize) -> Result<(usize, usize), String> {
    if requested_workers == 0 {
        return Err("workers must be greater than zero".into());
    }
    if work_items == 0 {
        return Ok((available_parallelism(), 0));
    }
    let available = available_parallelism();
    let effective = requested_workers
        .min(work_items)
        .min(available)
        .clamp(1, MAX_WORKERS);
    Ok((available, effective))
}

fn logical_id(index: usize, resident: usize, logical: u64) -> u64 {
    ((index as u128 * logical as u128) / resident as u128) as u64
}

/// Exact CPU implementation of GALAXY's existing split-u64 address mixer.
fn address_word(global_id: u64, seed: u32, lane: u32) -> u32 {
    let lo = global_id as u32;
    let hi = (global_id >> 32) as u32;
    let lane_key = lane.wrapping_add(1).wrapping_mul(0x9e37_79b9);
    let low_mix = hash32(lo ^ seed ^ lane_key ^ hash32(hi ^ 0xa511_e9b3));
    hash32(low_mix ^ hi.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b)
}

fn unit24(word: u32) -> f64 {
    (word >> 8) as f64 / 16_777_216.0
}

fn radians_to_bam(radians: f64) -> u32 {
    let turns = (radians / std::f64::consts::TAU).rem_euclid(1.0);
    let scaled = (turns * 4_294_967_296.0).round() as u64;
    scaled as u32
}

fn build_particles(logical: u64, resident: usize, seed: u32) -> Result<Vec<Particle>, String> {
    if resident == 0 || resident > MAX_RESIDENT || resident as u64 > logical {
        return Err("invalid logical/resident population".into());
    }
    let parameters = Parameters::default();
    let mut particles = Vec::with_capacity(resident);
    for index in 0..resident {
        let id = logical_id(index, resident, logical);
        let u = unit24(address_word(id, seed, 0));
        let kind = unit24(address_word(id, seed, 4));
        let radius = physics::star_radius(u, kind, BULGE);
        let rate = physics::angular_rate(radius, &parameters, SHEAR)
            .ok_or_else(|| format!("invalid physical rate for logical id {id}"))?;
        let radius_q16 = (radius * 65_536.0).round() as i32;
        let initial_bam = address_word(id, seed, 1);
        let initial_rad = initial_bam as f64 * TURN_SCALE;
        let delta_rad = rate * PHASE_STEP;
        let delta_bam = radians_to_bam(delta_rad);
        particles.push(Particle {
            id,
            radius_q16,
            initial_bam,
            delta_bam,
            initial_rad,
            delta_rad,
        });
    }
    Ok(particles)
}

fn contribution(id: u64, x: i64, y: i64) -> u64 {
    let lo = id as u32;
    let hi = (id >> 32) as u32;
    let first = hash32((x as u32) ^ hash32(y as u32) ^ lo ^ hash32(hi ^ 0xa511_e9b3));
    let second = hash32(first ^ hi ^ 0x85eb_ca6b);
    ((first as u64) << 32) | second as u64
}

fn project_particle(particle: &Particle, frame: usize, backend: Backend, lut: &Lut) -> u64 {
    let (x, y) = match backend {
        Backend::Float => {
            let angle = particle.initial_rad + particle.delta_rad * frame as f64;
            let (sin, cos) = angle.sin_cos();
            let radius = particle.radius_q16 as f64;
            ((radius * cos).round() as i64, (radius * sin).round() as i64)
        }
        Backend::Lut => {
            let angle = particle
                .initial_bam
                .wrapping_add(particle.delta_bam.wrapping_mul(frame as u32));
            let (cos, sin) = lut.sin_cos(angle);
            let radius = particle.radius_q16 as i64;
            ((radius * cos) >> 30, (radius * sin) >> 30)
        }
    };
    contribution(particle.id, x, y)
}

fn execute_range(particles: &[Particle], frames: usize, backend: Backend, lut: &Lut) -> u64 {
    let mut checksum = 0_u64;
    for frame in 0..frames {
        for particle in particles {
            checksum = checksum.wrapping_add(project_particle(particle, frame, backend, lut));
        }
    }
    checksum
}

fn execute_scalar(particles: &[Particle], frames: usize, backend: Backend, lut: &Lut) -> u64 {
    execute_range(particles, frames, backend, lut)
}

fn execute_parallel(
    particles: &[Particle],
    frames: usize,
    backend: Backend,
    lut: &Lut,
    workers: usize,
) -> Result<(u64, usize, usize), String> {
    let (available, worker_count) = effective_workers(particles.len(), workers)?;
    if worker_count <= 1 {
        return Ok((execute_scalar(particles, frames, backend, lut), available, worker_count));
    }

    let base_len = particles.len() / worker_count;
    let remainder = particles.len() % worker_count;
    let checksum = thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        let mut start = 0;
        for worker_index in 0..worker_count {
            let chunk_len = base_len + usize::from(worker_index < remainder);
            let end = start + chunk_len;
            let chunk = &particles[start..end];
            handles.push(scope.spawn(move || execute_range(chunk, frames, backend, lut)));
            start = end;
        }
        let mut total = 0_u64;
        for handle in handles {
            let chunk = handle
                .join()
                .map_err(|_| "GALAXY CPU worker thread panicked".to_string())?;
            total = total.wrapping_add(chunk);
        }
        Ok::<u64, String>(total)
    })?;
    Ok((checksum, available, worker_count))
}

fn median_sorted(values: &[u128]) -> u128 {
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        let lower = values[middle - 1];
        let upper = values[middle];
        lower + (upper - lower) / 2
    } else {
        values[middle]
    }
}

fn measure<F>(repeats: usize, mut function: F) -> Result<Timing, String>
where
    F: FnMut() -> Result<u64, String>,
{
    let mut timings = Vec::with_capacity(repeats);
    let mut checksum = None;
    for _ in 0..repeats {
        let started = Instant::now();
        let result = function()?;
        let elapsed = started.elapsed().as_nanos();
        let result = black_box(result);
        if let Some(expected) = checksum {
            if result != expected {
                return Err("runtime checksum changed between repeats".into());
            }
        } else {
            checksum = Some(result);
        }
        timings.push(elapsed);
    }
    timings.sort_unstable();
    Ok(Timing {
        best_ns: timings[0],
        median_ns: median_sorted(&timings),
        checksum: checksum.expect("positive repeat count"),
    })
}

fn measure_backend(
    particles: &[Particle],
    config: &Config,
    backend: Backend,
    lut: &Lut,
) -> Result<(BackendEvidence, usize, usize), String> {
    let (available, effective) = effective_workers(particles.len(), config.requested_workers)?;

    black_box(execute_scalar(
        &particles[..particles.len().min(4096)],
        config.frames.min(2),
        backend,
        lut,
    ));
    let scalar = measure(config.repeats, || {
        Ok(execute_scalar(particles, config.frames, backend, lut))
    })?;
    let parallel = measure(config.repeats, || {
        execute_parallel(particles, config.frames, backend, lut, config.requested_workers)
            .map(|(checksum, _, _)| checksum)
    })?;
    let checksum_match = scalar.checksum == parallel.checksum;
    if !checksum_match {
        return Err(format!(
            "{} scalar/parallel checksum mismatch: {:016x} != {:016x}",
            backend.name(), scalar.checksum, parallel.checksum
        ));
    }
    let speedup = scalar.median_ns as f64 / parallel.median_ns.max(1) as f64;
    let multicore_claim = effective > 1 && available > 1 && checksum_match && speedup > 1.0;
    Ok((
        BackendEvidence {
            scalar,
            parallel,
            speedup,
            checksum_match,
            multicore_claim,
        },
        available,
        effective,
    ))
}

fn lut_error(lut: &Lut) -> i64 {
    let mut maximum = 0_i64;
    for index in 0..8192_u32 {
        let angle = hash32(index.wrapping_mul(0x9e37_79b9));
        let (reference_cos, reference_sin) = sin_cos_q30(angle);
        let (lut_cos, lut_sin) = lut.sin_cos(angle);
        maximum = maximum.max((reference_cos - lut_cos).abs());
        maximum = maximum.max((reference_sin - lut_sin).abs());
    }
    maximum
}

fn json_bool(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn receipt_json(
    config: &Config,
    available: usize,
    effective: usize,
    lut_error_q30: i64,
    float: BackendEvidence,
    lut: BackendEvidence,
) -> String {
    format!(
        "{{\n  \"schema\": \"{RECEIPT_SCHEMA}\",\n  \"runtime\": \"galaxy-cpu\",\n  \"architecture\": \"{}\",\n  \"os\": \"{}\",\n  \"addressing\": \"{ADDRESSING}\",\n  \"logical_population\": \"{}\",\n  \"resident_particles\": {},\n  \"frames\": {},\n  \"repeats\": {},\n  \"seed\": {},\n  \"requested_workers\": {},\n  \"available_parallelism\": {},\n  \"effective_workers\": {},\n  \"lut_entries\": {},\n  \"lut_max_abs_q30_error\": {},\n  \"float\": {{\n    \"scalar_median_ns\": {},\n    \"parallel_median_ns\": {},\n    \"scalar_checksum\": \"{:016x}\",\n    \"parallel_checksum\": \"{:016x}\",\n    \"checksum_match\": {},\n    \"measured_speedup\": {:.9},\n    \"effective_multicore_claim\": {}\n  }},\n  \"bam_lut\": {{\n    \"scalar_median_ns\": {},\n    \"parallel_median_ns\": {},\n    \"scalar_checksum\": \"{:016x}\",\n    \"parallel_checksum\": \"{:016x}\",\n    \"checksum_match\": {},\n    \"measured_speedup\": {:.9},\n    \"effective_multicore_claim\": {}\n  }},\n  \"claim_boundary\": \"Environment-specific deterministic implementation evidence; not a universal multicore or end-to-end rendering performance claim.\"\n}}\n",
        env::consts::ARCH,
        env::consts::OS,
        config.logical,
        config.resident,
        config.frames,
        config.repeats,
        config.seed,
        config.requested_workers,
        available,
        effective,
        LUT_SIZE,
        lut_error_q30,
        float.scalar.median_ns,
        float.parallel.median_ns,
        float.scalar.checksum,
        float.parallel.checksum,
        json_bool(float.checksum_match),
        float.speedup,
        json_bool(float.multicore_claim),
        lut.scalar.median_ns,
        lut.parallel.median_ns,
        lut.scalar.checksum,
        lut.parallel.checksum,
        json_bool(lut.checksum_match),
        lut.speedup,
        json_bool(lut.multicore_claim),
    )
}

fn write_receipt(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create receipt directory: {error}"))?;
        }
    }
    fs::write(path, contents).map_err(|error| format!("cannot write receipt: {error}"))
}

fn print_timing(label: &str, timing: Timing, work: u128) {
    let per_second = if timing.median_ns == 0 {
        f64::INFINITY
    } else {
        work as f64 * 1e9 / timing.median_ns as f64
    };
    println!(
        "{label},best_ns={},median_ns={},work_per_second={per_second:.3},checksum={:016x}",
        timing.best_ns, timing.median_ns, timing.checksum
    );
}

fn run_bench(config: Config) -> Result<(), String> {
    println!("galaxy_cpu_runtime=v1");
    println!("arch={}", env::consts::ARCH);
    println!("os={}", env::consts::OS);
    println!("logical_population={}", config.logical);
    println!("resident_particles={}", config.resident);
    println!("frames={}", config.frames);
    println!("requested_workers={}", config.requested_workers);
    println!("repeats={}", config.repeats);
    println!("addressing={ADDRESSING}");

    let build_started = Instant::now();
    let particles = build_particles(config.logical, config.resident, config.seed)?;
    println!("resident_build_ns={}", build_started.elapsed().as_nanos());
    let lut = Lut::build();
    let lut_error_q30 = lut_error(&lut);
    println!("lut_entries={LUT_SIZE}");
    println!("lut_max_abs_q30_error={lut_error_q30}");

    let (float, available, effective) = measure_backend(&particles, &config, Backend::Float, &lut)?;
    let (lut_evidence, available_lut, effective_lut) =
        measure_backend(&particles, &config, Backend::Lut, &lut)?;
    if available != available_lut || effective != effective_lut {
        return Err("worker capacity changed during benchmark".into());
    }

    println!("available_parallelism={available}");
    println!("effective_workers={effective}");
    let work = config.resident as u128 * config.frames as u128;
    print_timing("float_scalar", float.scalar, work);
    print_timing("float_parallel", float.parallel, work);
    println!("float_parallel_speedup={:.9}", float.speedup);
    println!("float_effective_multicore_claim={}", float.multicore_claim);
    print_timing("bam_lut_scalar", lut_evidence.scalar, work);
    print_timing("bam_lut_parallel", lut_evidence.parallel, work);
    println!("bam_lut_parallel_speedup={:.9}", lut_evidence.speedup);
    println!("bam_lut_effective_multicore_claim={}", lut_evidence.multicore_claim);
    println!(
        "lut_vs_float_scalar_median_ratio={:.9}",
        float.scalar.median_ns as f64 / lut_evidence.scalar.median_ns.max(1) as f64
    );
    println!(
        "lut_vs_float_parallel_median_ratio={:.9}",
        float.parallel.median_ns as f64 / lut_evidence.parallel.median_ns.max(1) as f64
    );

    let receipt = receipt_json(
        &config,
        available,
        effective,
        lut_error_q30,
        float,
        lut_evidence,
    );
    if let Some(path) = &config.receipt {
        write_receipt(path, &receipt)?;
        println!("receipt={}", path.display());
    }
    Ok(())
}

fn run_verify(requested_workers: usize) -> Result<(), String> {
    let ids = [u32::MAX as u64, 1_u64 << 32, (1_u64 << 32) + 1];
    let words = ids.map(|id| address_word(id, DEFAULT_SEED, 0));
    if words[0] == words[1] || words[1] == words[2] || words[0] == words[2] {
        return Err("split-u64 address mixer aliased across the 2^32 boundary".into());
    }

    let resident = 65_536;
    let logical = u64::MAX;
    let first = logical_id(0, resident, logical);
    let middle = logical_id(resident / 2, resident, logical);
    let last = logical_id(resident - 1, resident, logical);
    if first != 0 || !(first < middle && middle < last) || last <= (1_u64 << 63) {
        return Err("u64 logical sampling did not span the population monotonically".into());
    }

    let particles = build_particles(logical, resident, DEFAULT_SEED)?;
    let lut = Lut::build();
    let error = lut_error(&lut);
    if error > 512 {
        return Err(format!("BAM LUT Q2.30 error exceeded bound: {error}"));
    }

    for backend in [Backend::Float, Backend::Lut] {
        let scalar = execute_scalar(&particles, 3, backend, &lut);
        let (parallel, available, effective) =
            execute_parallel(&particles, 3, backend, &lut, requested_workers)?;
        if scalar != parallel {
            return Err(format!("{} scalar/parallel checksum mismatch", backend.name()));
        }
        println!(
            "verify_backend={},checksum={scalar:016x},available_parallelism={available},effective_workers={effective}",
            backend.name()
        );
    }

    println!(
        "verify_u64_boundary={:08x},{:08x},{:08x}",
        words[0], words[1], words[2]
    );
    println!("verify_lut_max_abs_q30_error={error}");
    println!("GALAXY native CPU runtime verification passed");
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("verify") if help_requested(&args[1..]) => {
            print!("{}", usage());
            Ok(())
        }
        Some("verify") => parse_config(&args[1..]).and_then(|config| run_verify(config.requested_workers)),
        Some("bench") | Some("run") if help_requested(&args[1..]) => {
            print!("{}", usage());
            Ok(())
        }
        Some("bench") | Some("run") => parse_config(&args[1..]).and_then(run_bench),
        Some("--help") | Some("-h") | None => {
            print!("{}", usage());
            Ok(())
        }
        Some(command) => Err(format!("unknown command: {command}\n{}", usage())),
    };
    if let Err(error) = result {
        eprintln!("galaxy-cpu: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_u64_addressing_distinguishes_u32_boundary() {
        let a = address_word(u32::MAX as u64, 303, 0);
        let b = address_word(1_u64 << 32, 303, 0);
        let c = address_word((1_u64 << 32) + 1, 303, 0);
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn logical_sampling_uses_the_full_u64_address_space() {
        let resident = 1024;
        let logical = u64::MAX;
        let ids: Vec<_> = (0..resident)
            .map(|index| logical_id(index, resident, logical))
            .collect();
        assert_eq!(ids[0], 0);
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(ids[resident - 1] > (1_u64 << 63));
    }

    #[test]
    fn worker_resolution_never_exceeds_capacity_or_work() {
        let (available, effective) = effective_workers(7, 256).unwrap();
        assert!((1..=MAX_WORKERS).contains(&available));
        assert!((1..=7).contains(&effective));
        assert!(effective <= available);
        assert!(effective_workers(7, 0).is_err());
    }

    #[test]
    fn scalar_and_parallel_checksums_match() {
        let particles = build_particles(1_u64 << 40, 4096, 303).unwrap();
        let lut = Lut::build();
        for backend in [Backend::Float, Backend::Lut] {
            let scalar = execute_scalar(&particles, 3, backend, &lut);
            let (parallel, _, _) = execute_parallel(&particles, 3, backend, &lut, 4).unwrap();
            assert_eq!(scalar, parallel);
        }
    }

    #[test]
    fn help_flags_are_recognized_as_control_flow() {
        assert!(help_requested(&["--help".into()]));
        assert!(help_requested(&["--workers".into(), "4".into(), "-h".into()]));
        assert!(!help_requested(&["--workers".into(), "4".into()]));
    }

    #[test]
    fn receipt_includes_repeat_count() {
        let config = Config {
            logical: u64::MAX,
            resident: 65_536,
            frames: 4,
            requested_workers: 4,
            repeats: 7,
            seed: 303,
            receipt: None,
        };
        let timing = Timing {
            best_ns: 10,
            median_ns: 12,
            checksum: 0x1234,
        };
        let evidence = BackendEvidence {
            scalar: timing,
            parallel: timing,
            speedup: 1.0,
            checksum_match: true,
            multicore_claim: false,
        };
        let receipt = receipt_json(&config, 4, 4, 234, evidence, evidence);
        assert!(receipt.contains("\"repeats\": 7"));
    }
}
