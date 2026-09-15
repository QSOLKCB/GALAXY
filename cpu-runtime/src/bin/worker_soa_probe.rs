// SPDX-License-Identifier: Apache-2.0
//! Experimental worker-local SoA execution path for GALAXY's BAM-LUT CPU backend.
//!
//! This binary is deliberately separate from the canonical `galaxy-cpu` runtime.
//! It compares the current full-resident AoS shape with a bounded worker-local
//! structure-of-arrays tile that stages projection words and calls a SIMD-friendly
//! contribution batch. Both paths use the same deterministic addressing, physics,
//! LUT interpolation, contribution hash, contiguous worker partitioning, and
//! wrapping-u64 reduction contract.

use galaxy_retro_math::{hash32, sin_cos_q30};
use galaxy_sampler::physics::{self, Parameters};
use std::{
    env, fs,
    hint::black_box,
    mem::size_of,
    path::{Path, PathBuf},
    thread,
    time::Instant,
};

const RECEIPT_SCHEMA: &str = "galaxy.cpu-worker-soa-receipt.v1";
const ADDRESSING: &str = "split-u64-hash32-avalanche-v1";
const MAX_WORKERS: usize = 256;
const MAX_RESIDENT: usize = 16_777_216;
const MAX_FRAMES: usize = 100_000;
const MAX_REPEATS: usize = 25;
const MAX_TILE: usize = 1_048_576;
const DEFAULT_RESIDENT: usize = 262_144;
const DEFAULT_FRAMES: usize = 8;
const DEFAULT_REPEATS: usize = 3;
const DEFAULT_TILE: usize = 4_096;
const DEFAULT_SEED: u32 = 303;
const BULGE: f64 = 0.18;
const SHEAR: f64 = 0.35;
const PHASE_STEP: f64 = 1.0 / 60.0;
const TURN_SCALE: f64 = std::f64::consts::TAU / 4_294_967_296.0;

const LUT_BITS: u32 = 14;
const LUT_SIZE: usize = 1 << LUT_BITS;
const LUT_SHIFT: u32 = 32 - LUT_BITS;
const LUT_FRAC_MASK: u32 = (1_u32 << LUT_SHIFT) - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExecutionPath {
    Reference,
    WorkerSoa,
}

impl ExecutionPath {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "reference" => Ok(Self::Reference),
            "soa" | "worker-soa" => Ok(Self::WorkerSoa),
            _ => Err("--path must be reference or soa".into()),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Reference => "reference-aos",
            Self::WorkerSoa => "worker-local-soa",
        }
    }
}

/// Mirrors the canonical runtime's 40-byte resident particle shape.
#[derive(Clone, Copy, Debug)]
struct ReferenceParticle {
    id: u64,
    radius_q16: i32,
    initial_bam: u32,
    delta_bam: u32,
    _initial_rad: f64,
    _delta_rad: f64,
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

    #[inline(always)]
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

#[derive(Debug)]
struct CompactTile {
    id_lo: Vec<u32>,
    id_hi: Vec<u32>,
    radius_q16: Vec<i32>,
    initial_bam: Vec<u32>,
    delta_bam: Vec<u32>,
    x_word: Vec<u32>,
    y_word: Vec<u32>,
    contributions: Vec<u64>,
}

impl CompactTile {
    fn new(capacity: usize) -> Self {
        Self {
            id_lo: Vec::with_capacity(capacity),
            id_hi: Vec::with_capacity(capacity),
            radius_q16: Vec::with_capacity(capacity),
            initial_bam: Vec::with_capacity(capacity),
            delta_bam: Vec::with_capacity(capacity),
            x_word: Vec::with_capacity(capacity),
            y_word: Vec::with_capacity(capacity),
            contributions: Vec::with_capacity(capacity),
        }
    }

    fn clear(&mut self) {
        self.id_lo.clear();
        self.id_hi.clear();
        self.radius_q16.clear();
        self.initial_bam.clear();
        self.delta_bam.clear();
        self.x_word.clear();
        self.y_word.clear();
        self.contributions.clear();
    }

    fn resize_scratch(&mut self, len: usize) {
        self.x_word.resize(len, 0);
        self.y_word.resize(len, 0);
        self.contributions.resize(len, 0);
    }
}

#[derive(Clone, Debug)]
struct Config {
    path: ExecutionPath,
    logical: u64,
    resident: usize,
    frames: usize,
    requested_workers: usize,
    tile_particles: usize,
    repeats: usize,
    seed: u32,
    receipt: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
struct Timing {
    best_ns: u128,
    median_ns: u128,
    checksum: u64,
}

#[derive(Clone, Copy, Debug)]
struct Measurement {
    timing: Timing,
    available_parallelism: usize,
    effective_workers: usize,
    peak_rss_kib: Option<u64>,
}

fn usage() -> &'static str {
    "Usage:\n  worker_soa_probe verify [--workers N] [--tile N]\n  worker_soa_probe bench [--path reference|soa] [--logical U64] [--resident N] [--frames N] [--workers N] [--tile N] [--repeats N] [--seed U32] [--receipt PATH]\n\nDefaults:\n  path=reference logical=18446744073709551615 resident=262144 frames=8 repeats=3 tile=4096 seed=303\n  workers=min(std::thread::available_parallelism(), 256)\n"
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

fn available_parallelism() -> usize {
    thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .max(1)
}

fn default_requested_workers() -> usize {
    available_parallelism().min(MAX_WORKERS)
}

fn parse_config(args: &[String]) -> Result<Config, String> {
    let mut config = Config {
        path: ExecutionPath::Reference,
        logical: u64::MAX,
        resident: DEFAULT_RESIDENT,
        frames: DEFAULT_FRAMES,
        requested_workers: default_requested_workers(),
        tile_particles: DEFAULT_TILE,
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
            "--path" => config.path = ExecutionPath::parse(&value)?,
            "--logical" => {
                config.logical = value
                    .parse::<u64>()
                    .map_err(|_| "--logical must be an exact decimal u64".to_string())?;
                if config.logical == 0 {
                    return Err("--logical must be greater than zero".into());
                }
            }
            "--resident" => config.resident = parse_positive(flag, value, MAX_RESIDENT)?,
            "--frames" => config.frames = parse_positive(flag, value, MAX_FRAMES)?,
            "--workers" => config.requested_workers = parse_positive(flag, value, MAX_WORKERS)?,
            "--tile" => config.tile_particles = parse_positive(flag, value, MAX_TILE)?,
            "--repeats" => config.repeats = parse_positive(flag, value, MAX_REPEATS)?,
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

fn parse_verify(args: &[String]) -> Result<(usize, usize), String> {
    let mut workers = default_requested_workers();
    let mut tile = DEFAULT_TILE;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("{flag} requires a value"))?
            .clone();
        match flag {
            "--workers" => workers = parse_positive(flag, value, MAX_WORKERS)?,
            "--tile" => tile = parse_positive(flag, value, MAX_TILE)?,
            _ => return Err(format!("verify only accepts --workers N and --tile N; got {flag}")),
        }
        index += 2;
    }
    Ok((workers, tile))
}

fn effective_workers(work_items: usize, requested_workers: usize) -> (usize, usize) {
    let available = available_parallelism();
    let effective = requested_workers
        .min(work_items.max(1))
        .min(available)
        .min(MAX_WORKERS)
        .max(1);
    (available, effective)
}

fn logical_id(index: usize, resident: usize, logical: u64) -> u64 {
    ((index as u128 * logical as u128) / resident as u128) as u64
}

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

fn particle_fields(
    index: usize,
    resident: usize,
    logical: u64,
    seed: u32,
) -> Result<(u64, i32, u32, u32, f64, f64), String> {
    let id = logical_id(index, resident, logical);
    let u = unit24(address_word(id, seed, 0));
    let kind = unit24(address_word(id, seed, 4));
    let radius = physics::star_radius(u, kind, BULGE);
    let rate = physics::angular_rate(radius, &Parameters::default(), SHEAR)
        .ok_or_else(|| format!("invalid physical rate for logical id {id}"))?;
    let radius_q16 = (radius * 65_536.0).round() as i32;
    let initial_bam = address_word(id, seed, 1);
    let initial_rad = initial_bam as f64 * TURN_SCALE;
    let delta_rad = rate * PHASE_STEP;
    let delta_bam = radians_to_bam(delta_rad);
    Ok((
        id,
        radius_q16,
        initial_bam,
        delta_bam,
        initial_rad,
        delta_rad,
    ))
}

fn build_reference_particles(config: &Config) -> Result<Vec<ReferenceParticle>, String> {
    let mut particles = Vec::with_capacity(config.resident);
    for index in 0..config.resident {
        let (id, radius_q16, initial_bam, delta_bam, initial_rad, delta_rad) =
            particle_fields(index, config.resident, config.logical, config.seed)?;
        particles.push(ReferenceParticle {
            id,
            radius_q16,
            initial_bam,
            delta_bam,
            _initial_rad: initial_rad,
            _delta_rad: delta_rad,
        });
    }
    Ok(particles)
}

#[inline(always)]
fn contribution_reference(id: u64, x: i64, y: i64) -> u64 {
    let lo = id as u32;
    let hi = (id >> 32) as u32;
    let first = hash32((x as u32) ^ hash32(y as u32) ^ lo ^ hash32(hi ^ 0xa511_e9b3));
    let second = hash32(first ^ hi ^ 0x85eb_ca6b);
    ((first as u64) << 32) | second as u64
}

#[inline(always)]
fn hash32_local(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

/// SIMD-friendly SoA form of the exact GALAXY contribution hash.
///
/// This is the PR #10 batch shape, now called by the worker-local runtime path.
/// Keeping the symbol unmangled makes the production-path experiment directly
/// inspectable with `objdump --disassemble=galaxy_worker_soa_hash_batch`.
#[no_mangle]
#[inline(never)]
pub fn galaxy_worker_soa_hash_batch(
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

fn execute_reference_range(particles: &[ReferenceParticle], frames: usize, lut: &Lut) -> u64 {
    let mut checksum = 0_u64;
    for frame in 0..frames {
        for particle in particles {
            let angle = particle
                .initial_bam
                .wrapping_add(particle.delta_bam.wrapping_mul(frame as u32));
            let (cos, sin) = lut.sin_cos(angle);
            let radius = particle.radius_q16 as i64;
            let x = (radius * cos) >> 30;
            let y = (radius * sin) >> 30;
            checksum = checksum.wrapping_add(contribution_reference(particle.id, x, y));
        }
    }
    checksum
}

fn execute_reference(config: &Config, lut: &Lut, worker_count: usize) -> Result<u64, String> {
    let particles = build_reference_particles(config)?;
    if worker_count == 1 {
        return Ok(execute_reference_range(&particles, config.frames, lut));
    }
    let base_len = particles.len() / worker_count;
    let remainder = particles.len() % worker_count;
    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        let mut start = 0;
        for worker_index in 0..worker_count {
            let chunk_len = base_len + usize::from(worker_index < remainder);
            let end = start + chunk_len;
            let chunk = &particles[start..end];
            handles.push(scope.spawn(move || execute_reference_range(chunk, config.frames, lut)));
            start = end;
        }
        let mut total = 0_u64;
        for handle in handles {
            total = total.wrapping_add(
                handle
                    .join()
                    .map_err(|_| "reference worker thread panicked".to_string())?,
            );
        }
        Ok(total)
    })
}

fn fill_compact_tile(
    tile: &mut CompactTile,
    start: usize,
    end: usize,
    config: &Config,
) -> Result<(), String> {
    tile.clear();
    for index in start..end {
        let (id, radius_q16, initial_bam, delta_bam, _, _) =
            particle_fields(index, config.resident, config.logical, config.seed)?;
        tile.id_lo.push(id as u32);
        tile.id_hi.push((id >> 32) as u32);
        tile.radius_q16.push(radius_q16);
        tile.initial_bam.push(initial_bam);
        tile.delta_bam.push(delta_bam);
    }
    tile.resize_scratch(end - start);
    Ok(())
}

fn execute_soa_worker(
    start: usize,
    end: usize,
    config: &Config,
    lut: &Lut,
) -> Result<u64, String> {
    let capacity = config.tile_particles.min(end.saturating_sub(start)).max(1);
    let mut tile = CompactTile::new(capacity);
    let mut checksum = 0_u64;
    let mut tile_start = start;
    while tile_start < end {
        let tile_end = (tile_start + config.tile_particles).min(end);
        fill_compact_tile(&mut tile, tile_start, tile_end, config)?;
        let len = tile.id_lo.len();
        for frame in 0..config.frames {
            for index in 0..len {
                let angle = tile.initial_bam[index]
                    .wrapping_add(tile.delta_bam[index].wrapping_mul(frame as u32));
                let (cos, sin) = lut.sin_cos(angle);
                let radius = tile.radius_q16[index] as i64;
                tile.x_word[index] = (((radius * cos) >> 30) as i64) as u32;
                tile.y_word[index] = (((radius * sin) >> 30) as i64) as u32;
            }
            galaxy_worker_soa_hash_batch(
                &tile.id_lo,
                &tile.id_hi,
                &tile.x_word,
                &tile.y_word,
                &mut tile.contributions,
            );
            for &value in &tile.contributions {
                checksum = checksum.wrapping_add(value);
            }
        }
        tile_start = tile_end;
    }
    Ok(checksum)
}

fn partition(total: usize, worker_count: usize, worker_index: usize) -> (usize, usize) {
    let base = total / worker_count;
    let remainder = total % worker_count;
    let extra_before = worker_index.min(remainder);
    let start = worker_index * base + extra_before;
    let len = base + usize::from(worker_index < remainder);
    (start, start + len)
}

fn soa_worker_tile_capacity_particles(
    resident: usize,
    worker_count: usize,
    tile_particles: usize,
) -> usize {
    (0..worker_count).fold(0_usize, |total, worker_index| {
        let (start, end) = partition(resident, worker_count, worker_index);
        total.saturating_add(tile_particles.min(end.saturating_sub(start)))
    })
}

fn execute_soa(config: &Config, lut: &Lut, worker_count: usize) -> Result<u64, String> {
    if worker_count == 1 {
        return execute_soa_worker(0, config.resident, config, lut);
    }
    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            let (start, end) = partition(config.resident, worker_count, worker_index);
            handles.push(scope.spawn(move || execute_soa_worker(start, end, config, lut)));
        }
        let mut total = 0_u64;
        for handle in handles {
            total = total.wrapping_add(
                handle
                    .join()
                    .map_err(|_| "worker-local SoA thread panicked".to_string())??,
            );
        }
        Ok(total)
    })
}

fn execute_path(config: &Config, lut: &Lut, worker_count: usize) -> Result<u64, String> {
    match config.path {
        ExecutionPath::Reference => execute_reference(config, lut, worker_count),
        ExecutionPath::WorkerSoa => execute_soa(config, lut, worker_count),
    }
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

fn read_peak_rss_kib() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<u64>().ok());
        }
    }
    None
}

fn measure(config: &Config, lut: &Lut) -> Result<Measurement, String> {
    let (available, effective) = effective_workers(config.resident, config.requested_workers);
    let warm = black_box(execute_path(config, lut, effective)?);
    let mut timings = Vec::with_capacity(config.repeats);
    let mut checksum = None;
    for _ in 0..config.repeats {
        let started = Instant::now();
        let result = black_box(execute_path(config, lut, effective)?);
        let elapsed = started.elapsed().as_nanos();
        if let Some(expected) = checksum {
            if result != expected {
                return Err("checksum changed between repeated end-to-end trials".into());
            }
        } else {
            checksum = Some(result);
        }
        if result != warm {
            return Err("warm-up and measured checksum disagree".into());
        }
        timings.push(elapsed);
    }
    timings.sort_unstable();
    Ok(Measurement {
        timing: Timing {
            best_ns: timings[0],
            median_ns: median_sorted(&timings),
            checksum: checksum.expect("positive repeat count"),
        },
        available_parallelism: available,
        effective_workers: effective,
        peak_rss_kib: read_peak_rss_kib(),
    })
}

fn json_optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "null".into(), |value| value.to_string())
}

fn receipt_json(config: &Config, measurement: Measurement) -> String {
    let compact_field_bytes = 5 * size_of::<u32>();
    let scratch_bytes = 2 * size_of::<u32>() + size_of::<u64>();
    let worker_tile_capacity_particles = match config.path {
        ExecutionPath::Reference => 0,
        ExecutionPath::WorkerSoa => soa_worker_tile_capacity_particles(
            config.resident,
            measurement.effective_workers,
            config.tile_particles,
        ),
    };
    let worker_tile_capacity_bytes = (compact_field_bytes + scratch_bytes)
        .saturating_mul(worker_tile_capacity_particles);
    format!(
        "{{\n  \"schema\": \"{RECEIPT_SCHEMA}\",\n  \"runtime\": \"worker-soa-prototype\",\n  \"execution_path\": \"{}\",\n  \"architecture\": \"{}\",\n  \"os\": \"{}\",\n  \"addressing\": \"{ADDRESSING}\",\n  \"logical_population\": \"{}\",\n  \"resident_particles\": {},\n  \"frames\": {},\n  \"repeats\": {},\n  \"seed\": {},\n  \"requested_workers\": {},\n  \"available_parallelism\": {},\n  \"effective_workers\": {},\n  \"tile_particles\": {},\n  \"reference_particle_bytes\": {},\n  \"soa_compact_field_bytes_per_particle\": {},\n  \"soa_scratch_bytes_per_particle\": {},\n  \"soa_worker_tile_capacity_bytes\": {},\n  \"best_ns\": {},\n  \"median_ns\": {},\n  \"checksum\": \"{:016x}\",\n  \"peak_rss_kib\": {},\n  \"resident_generation_in_timed_region\": true,\n  \"timing_scope\": \"resident-generation-plus-bam-lut-projection-plus-contribution-plus-worker-reduction\",\n  \"claim_boundary\": \"Experimental end-to-end BAM-LUT path evidence. This receipt does not replace galaxy-cpu and does not establish a universal production speedup. RSS is Linux VmHWM when available, otherwise null.\"\n}}\n",
        config.path.name(),
        env::consts::ARCH,
        env::consts::OS,
        config.logical,
        config.resident,
        config.frames,
        config.repeats,
        config.seed,
        config.requested_workers,
        measurement.available_parallelism,
        measurement.effective_workers,
        config.tile_particles,
        size_of::<ReferenceParticle>(),
        compact_field_bytes,
        scratch_bytes,
        worker_tile_capacity_bytes,
        measurement.timing.best_ns,
        measurement.timing.median_ns,
        measurement.timing.checksum,
        json_optional_u64(measurement.peak_rss_kib),
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

fn run_bench(config: Config) -> Result<(), String> {
    let lut = Lut::build();
    let measurement = measure(&config, &lut)?;
    println!("galaxy_cpu_worker_soa=v1");
    println!("execution_path={}", config.path.name());
    println!("logical_population={}", config.logical);
    println!("resident_particles={}", config.resident);
    println!("frames={}", config.frames);
    println!("requested_workers={}", config.requested_workers);
    println!("effective_workers={}", measurement.effective_workers);
    println!("tile_particles={}", config.tile_particles);
    println!("reference_particle_bytes={}", size_of::<ReferenceParticle>());
    println!("best_ns={}", measurement.timing.best_ns);
    println!("median_ns={}", measurement.timing.median_ns);
    println!("checksum={:016x}", measurement.timing.checksum);
    match measurement.peak_rss_kib {
        Some(value) => println!("peak_rss_kib={value}"),
        None => println!("peak_rss_kib=unavailable"),
    }
    if let Some(path) = &config.receipt {
        write_receipt(path, &receipt_json(&config, measurement))?;
        println!("receipt={}", path.display());
    }
    Ok(())
}

fn run_verify(requested_workers: usize, requested_tile: usize) -> Result<(), String> {
    if size_of::<ReferenceParticle>() != 40 {
        return Err(format!(
            "reference particle layout changed: expected 40 bytes, got {}",
            size_of::<ReferenceParticle>()
        ));
    }

    let config = Config {
        path: ExecutionPath::Reference,
        logical: u64::MAX,
        resident: 32_768,
        frames: 3,
        requested_workers,
        tile_particles: requested_tile,
        repeats: 1,
        seed: DEFAULT_SEED,
        receipt: None,
    };
    let lut = Lut::build();
    let (_, effective) = effective_workers(config.resident, requested_workers);
    let reference_particles = build_reference_particles(&config)?;
    let reference = execute_reference_range(&reference_particles, config.frames, &lut);

    let mut tiles = vec![
        1_usize,
        7,
        127,
        requested_tile.min(config.resident).max(1),
    ];
    tiles.sort_unstable();
    tiles.dedup();
    for tile in tiles {
        let mut soa_config = config.clone();
        soa_config.path = ExecutionPath::WorkerSoa;
        soa_config.tile_particles = tile;
        let soa = execute_soa(&soa_config, &lut, effective)?;
        if soa != reference {
            return Err(format!(
                "worker-local SoA checksum mismatch for tile {tile}: {soa:016x} != {reference:016x}"
            ));
        }
    }

    let reference_parallel = execute_reference(&config, &lut, effective)?;
    if reference_parallel != reference {
        return Err("reference scalar/parallel checksum mismatch".into());
    }

    let ids = [0_u64, 1_u64 << 32, u64::MAX - 1];
    let mut id_lo = Vec::new();
    let mut id_hi = Vec::new();
    let mut x = Vec::new();
    let mut y = Vec::new();
    let mut expected = Vec::new();
    for (index, id) in ids.into_iter().enumerate() {
        let xv = hash32(index as u32 ^ 0x1234_5678);
        let yv = hash32(index as u32 ^ 0x9abc_def0);
        id_lo.push(id as u32);
        id_hi.push((id >> 32) as u32);
        x.push(xv);
        y.push(yv);
        expected.push(contribution_reference(
            id,
            xv as i32 as i64,
            yv as i32 as i64,
        ));
    }
    let mut actual = vec![0_u64; ids.len()];
    galaxy_worker_soa_hash_batch(&id_lo, &id_hi, &x, &y, &mut actual);
    if actual != expected {
        return Err("SIMD batch contribution diverged from canonical contribution".into());
    }

    println!(
        "verify_reference_particle_bytes={}",
        size_of::<ReferenceParticle>()
    );
    println!("verify_effective_workers={effective}");
    println!("verify_checksum={reference:016x}");
    println!("GALAXY worker-local SoA prototype verification passed");
    Ok(())
}

fn help_requested(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--help" || arg == "-h")
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("verify") if help_requested(&args[1..]) => {
            print!("{}", usage());
            Ok(())
        }
        Some("verify") => {
            parse_verify(&args[1..]).and_then(|(workers, tile)| run_verify(workers, tile))
        }
        Some("bench") if help_requested(&args[1..]) => {
            print!("{}", usage());
            Ok(())
        }
        Some("bench") => parse_config(&args[1..]).and_then(run_bench),
        Some("--help") | Some("-h") | None => {
            print!("{}", usage());
            Ok(())
        }
        Some(command) => Err(format!("unknown command: {command}\n{}", usage())),
    };
    if let Err(error) = result {
        eprintln!("worker_soa_probe: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(path: ExecutionPath, resident: usize, workers: usize, tile: usize) -> Config {
        Config {
            path,
            logical: u64::MAX,
            resident,
            frames: 3,
            requested_workers: workers,
            tile_particles: tile,
            repeats: 1,
            seed: DEFAULT_SEED,
            receipt: None,
        }
    }

    #[test]
    fn reference_layout_matches_canonical_particle_size() {
        assert_eq!(size_of::<ReferenceParticle>(), 40);
    }

    #[test]
    fn worker_partition_covers_population_exactly() {
        let total = 101;
        let workers = 7;
        let mut cursor = 0;
        for worker in 0..workers {
            let (start, end) = partition(total, workers, worker);
            assert_eq!(start, cursor);
            assert!(end > start);
            cursor = end;
        }
        assert_eq!(cursor, total);
    }

    #[test]
    fn soa_tile_capacity_matches_actual_worker_allocations() {
        assert_eq!(soa_worker_tile_capacity_particles(1, 1, MAX_TILE), 1);
        assert_eq!(soa_worker_tile_capacity_particles(10, 4, 8), 10);
        assert_eq!(soa_worker_tile_capacity_particles(100, 4, 8), 32);
    }

    #[test]
    fn soa_matches_reference_across_tiles_and_workers() {
        let lut = Lut::build();
        for workers in [1_usize, 2, 4] {
            let reference_config = config(ExecutionPath::Reference, 4096, workers, 64);
            let (_, effective) = effective_workers(reference_config.resident, workers);
            let reference = execute_reference(&reference_config, &lut, effective).unwrap();
            for tile in [1_usize, 7, 64, 257, 4096] {
                let soa_config = config(ExecutionPath::WorkerSoa, 4096, workers, tile);
                let soa = execute_soa(&soa_config, &lut, effective).unwrap();
                assert_eq!(soa, reference, "workers={workers} tile={tile}");
            }
        }
    }

    #[test]
    fn soa_checksum_is_worker_count_invariant() {
        let lut = Lut::build();
        let base = config(ExecutionPath::WorkerSoa, 2048, 1, 127);
        let one = execute_soa(&base, &lut, 1).unwrap();
        for workers in [2_usize, 4, 8] {
            let (_, effective) = effective_workers(base.resident, workers);
            let value = execute_soa(&base, &lut, effective).unwrap();
            assert_eq!(value, one);
        }
    }

    #[test]
    fn batch_hash_matches_canonical_contribution() {
        let id_lo = [1_u32, 0, u32::MAX];
        let id_hi = [0_u32, 1, u32::MAX];
        let x = [0x1234_5678, 0xffff_fffe, 7];
        let y = [0x9abc_def0, 3, 0x8000_0000];
        let mut out = [0_u64; 3];
        galaxy_worker_soa_hash_batch(&id_lo, &id_hi, &x, &y, &mut out);
        for index in 0..out.len() {
            let id = ((id_hi[index] as u64) << 32) | id_lo[index] as u64;
            assert_eq!(
                out[index],
                contribution_reference(id, x[index] as i32 as i64, y[index] as i32 as i64)
            );
        }
    }

    #[test]
    fn receipt_reports_actual_soa_capacity_and_zero_for_reference() {
        let measurement = Measurement {
            timing: Timing {
                best_ns: 10,
                median_ns: 12,
                checksum: 0x1234,
            },
            available_parallelism: 32,
            effective_workers: 1,
            peak_rss_kib: Some(4096),
        };
        let soa_config = config(ExecutionPath::WorkerSoa, 1, 1, MAX_TILE);
        let soa_receipt = receipt_json(&soa_config, measurement);
        assert!(soa_receipt.contains("\"soa_worker_tile_capacity_bytes\": 36"));

        let reference_config = config(ExecutionPath::Reference, 1, 1, MAX_TILE);
        let reference_receipt = receipt_json(&reference_config, measurement);
        assert!(reference_receipt.contains("\"soa_worker_tile_capacity_bytes\": 0"));
    }

    #[test]
    fn receipt_labels_end_to_end_scope() {
        let config = config(ExecutionPath::WorkerSoa, 1024, 4, 128);
        let measurement = Measurement {
            timing: Timing {
                best_ns: 10,
                median_ns: 12,
                checksum: 0x1234,
            },
            available_parallelism: 32,
            effective_workers: 4,
            peak_rss_kib: Some(4096),
        };
        let receipt = receipt_json(&config, measurement);
        assert!(receipt.contains("\"execution_path\": \"worker-local-soa\""));
        assert!(receipt.contains("\"resident_generation_in_timed_region\": true"));
        assert!(receipt.contains("\"peak_rss_kib\": 4096"));
        assert!(receipt.contains("\"reference_particle_bytes\": 40"));
    }
}