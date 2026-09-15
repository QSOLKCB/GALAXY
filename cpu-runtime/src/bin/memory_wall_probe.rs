// SPDX-License-Identifier: Apache-2.0
//! PE #15 memory-wall probe: stream -> reduce -> discard.
//!
//! This binary is deliberately separate from the canonical `galaxy-cpu` runtime.
//! It compares the v0.5.0 materialized contribution-array shape with a fused
//! wrapping-u64 reduction, sweeps cache-sized microtiles, and evaluates exact
//! compressed BAM LUT representations.

mod memory_lut;

use galaxy_retro_math::hash32;
use galaxy_sampler::physics::{self, Parameters};
use memory_lut::{Lut, LutMode, LUT_SIZE};
use std::{
    env, fs,
    hint::black_box,
    mem::size_of,
    path::{Path, PathBuf},
    thread,
    time::Instant,
};

const RECEIPT_SCHEMA: &str = "galaxy.cpu-memory-wall-probe.v1";
const ADDRESSING: &str = "split-u64-hash32-avalanche-v1";
const MAX_WORKERS: usize = 256;
const MAX_RESIDENT: usize = 16_777_216;
const MAX_FRAMES: usize = 100_000;
const MAX_REPEATS: usize = 25;
const MAX_TILE: usize = 1_048_576;
const DEFAULT_RESIDENT: usize = 1_048_576;
const DEFAULT_FRAMES: usize = 8;
const DEFAULT_REPEATS: usize = 5;
const DEFAULT_TILE: usize = 128;
const DEFAULT_SEED: u32 = 303;
const BULGE: f64 = 0.18;
const SHEAR: f64 = 0.35;
const PHASE_STEP: f64 = 1.0 / 60.0;
const MICRO_TILES: [usize; 5] = [1_024, 512, 256, 128, 64];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReductionMode {
    Materialized,
    Fused,
}

impl ReductionMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "materialized" | "baseline" => Ok(Self::Materialized),
            "fused" | "stream" | "reduce" => Ok(Self::Fused),
            _ => Err("--reduce must be materialized or fused".into()),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Materialized => "materialized-contribution-array",
            Self::Fused => "fused-hash-reduce",
        }
    }

    fn materializes_contributions(self) -> bool {
        matches!(self, Self::Materialized)
    }

    fn scratch_bytes_per_particle(self) -> usize {
        let xy = 2 * size_of::<u32>();
        match self {
            Self::Materialized => xy + size_of::<u64>(),
            Self::Fused => xy,
        }
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
    contributions: Option<Vec<u64>>,
}

impl CompactTile {
    fn new(capacity: usize, reduction: ReductionMode) -> Self {
        Self {
            id_lo: Vec::with_capacity(capacity),
            id_hi: Vec::with_capacity(capacity),
            radius_q16: Vec::with_capacity(capacity),
            initial_bam: Vec::with_capacity(capacity),
            delta_bam: Vec::with_capacity(capacity),
            x_word: Vec::with_capacity(capacity),
            y_word: Vec::with_capacity(capacity),
            contributions: reduction
                .materializes_contributions()
                .then(|| Vec::with_capacity(capacity)),
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
        if let Some(values) = &mut self.contributions {
            values.clear();
        }
    }

    fn resize_scratch(&mut self, len: usize) {
        self.x_word.resize(len, 0);
        self.y_word.resize(len, 0);
        if let Some(values) = &mut self.contributions {
            values.resize(len, 0);
        }
    }
}

#[derive(Clone, Debug)]
struct Config {
    logical: u64,
    resident: usize,
    frames: usize,
    requested_workers: usize,
    tile_particles: usize,
    repeats: usize,
    seed: u32,
    reduction: ReductionMode,
    lut_mode: LutMode,
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
    "Usage:\n  memory_wall_probe verify [--workers N]\n  memory_wall_probe bench [--reduce materialized|fused] [--lut full|cosine|quarter] [--logical U64] [--resident N] [--frames N] [--workers N] [--tile N] [--repeats N] [--seed U32] [--receipt PATH]\n\nDefaults:\n  reduce=fused lut=full logical=18446744073709551615 resident=1048576 frames=8 repeats=5 tile=128 seed=303\n  workers=min(std::thread::available_parallelism(), 256)\n\nPE #15 microtile evidence set: 1024 512 256 128 64\n"
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
        logical: u64::MAX,
        resident: DEFAULT_RESIDENT,
        frames: DEFAULT_FRAMES,
        requested_workers: default_requested_workers(),
        tile_particles: DEFAULT_TILE,
        repeats: DEFAULT_REPEATS,
        seed: DEFAULT_SEED,
        reduction: ReductionMode::Fused,
        lut_mode: LutMode::Full,
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
            "--reduce" => config.reduction = ReductionMode::parse(&value)?,
            "--lut" => config.lut_mode = LutMode::parse(&value)?,
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
) -> Result<(u64, i32, u32, u32), String> {
    let id = logical_id(index, resident, logical);
    let u = unit24(address_word(id, seed, 0));
    let kind = unit24(address_word(id, seed, 4));
    let radius = physics::star_radius(u, kind, BULGE);
    let rate = physics::angular_rate(radius, &Parameters::default(), SHEAR)
        .ok_or_else(|| format!("invalid physical rate for logical id {id}"))?;
    let radius_q16 = (radius * 65_536.0).round() as i32;
    let initial_bam = address_word(id, seed, 1);
    let delta_bam = radians_to_bam(rate * PHASE_STEP);
    Ok((id, radius_q16, initial_bam, delta_bam))
}

#[inline(always)]
fn contribution_reference(id: u64, x: u32, y: u32) -> u64 {
    let lo = id as u32;
    let hi = (id >> 32) as u32;
    let first = hash32(x ^ hash32(y) ^ lo ^ hash32(hi ^ 0xa511_e9b3));
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

#[no_mangle]
#[inline(never)]
pub fn galaxy_memory_hash_batch(
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

#[no_mangle]
#[inline(never)]
pub fn galaxy_memory_hash_reduce(
    id_lo: &[u32],
    id_hi: &[u32],
    x: &[u32],
    y: &[u32],
) -> u64 {
    debug_assert_eq!(id_lo.len(), id_hi.len());
    debug_assert_eq!(id_lo.len(), x.len());
    debug_assert_eq!(id_lo.len(), y.len());
    let mut checksum = 0_u64;
    for index in 0..id_lo.len() {
        let first = hash32_local(
            x[index]
                ^ hash32_local(y[index])
                ^ id_lo[index]
                ^ hash32_local(id_hi[index] ^ 0xa511_e9b3),
        );
        let second = hash32_local(first ^ id_hi[index] ^ 0x85eb_ca6b);
        checksum = checksum.wrapping_add(((first as u64) << 32) | second as u64);
    }
    checksum
}

fn fill_tile(
    tile: &mut CompactTile,
    start: usize,
    end: usize,
    config: &Config,
) -> Result<(), String> {
    tile.clear();
    for index in start..end {
        let (id, radius_q16, initial_bam, delta_bam) =
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

fn execute_worker(
    start: usize,
    end: usize,
    config: &Config,
    lut: &Lut,
) -> Result<u64, String> {
    let capacity = config.tile_particles.min(end.saturating_sub(start)).max(1);
    let mut tile = CompactTile::new(capacity, config.reduction);
    let mut checksum = 0_u64;
    let mut tile_start = start;
    while tile_start < end {
        let tile_end = (tile_start + config.tile_particles).min(end);
        fill_tile(&mut tile, tile_start, tile_end, config)?;
        let len = tile.id_lo.len();
        for frame in 0..config.frames {
            for index in 0..len {
                let angle = tile.initial_bam[index]
                    .wrapping_add(tile.delta_bam[index].wrapping_mul(frame as u32));
                let (cos, sin) = lut.sin_cos(angle);
                let radius = tile.radius_q16[index] as i64;
                tile.x_word[index] = ((radius * cos) >> 30) as u32;
                tile.y_word[index] = ((radius * sin) >> 30) as u32;
            }
            match config.reduction {
                ReductionMode::Materialized => {
                    let values = tile
                        .contributions
                        .as_mut()
                        .ok_or_else(|| "materialized contribution scratch missing".to_string())?;
                    galaxy_memory_hash_batch(
                        &tile.id_lo,
                        &tile.id_hi,
                        &tile.x_word,
                        &tile.y_word,
                        values,
                    );
                    for &value in values.iter() {
                        checksum = checksum.wrapping_add(value);
                    }
                }
                ReductionMode::Fused => {
                    checksum = checksum.wrapping_add(galaxy_memory_hash_reduce(
                        &tile.id_lo,
                        &tile.id_hi,
                        &tile.x_word,
                        &tile.y_word,
                    ));
                }
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

fn execute(config: &Config, lut: &Lut, worker_count: usize) -> Result<u64, String> {
    if worker_count == 1 {
        return execute_worker(0, config.resident, config, lut);
    }
    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            let (start, end) = partition(config.resident, worker_count, worker_index);
            handles.push(scope.spawn(move || execute_worker(start, end, config, lut)));
        }
        let mut total = 0_u64;
        for handle in handles {
            total = total.wrapping_add(
                handle
                    .join()
                    .map_err(|_| "memory-wall worker panicked".to_string())??,
            );
        }
        Ok(total)
    })
}

fn streaming_reference_checksum(config: &Config, lut: &Lut) -> Result<u64, String> {
    let mut checksum = 0_u64;
    for index in 0..config.resident {
        let (id, radius_q16, initial_bam, delta_bam) =
            particle_fields(index, config.resident, config.logical, config.seed)?;
        let radius = radius_q16 as i64;
        for frame in 0..config.frames {
            let angle = initial_bam.wrapping_add(delta_bam.wrapping_mul(frame as u32));
            let (cos, sin) = lut.sin_cos(angle);
            let x = ((radius * cos) >> 30) as u32;
            let y = ((radius * sin) >> 30) as u32;
            checksum = checksum.wrapping_add(contribution_reference(id, x, y));
        }
    }
    Ok(checksum)
}

fn median_sorted(values: &[u128]) -> u128 {
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        values[middle - 1] + (values[middle] - values[middle - 1]) / 2
    } else {
        values[middle]
    }
}

fn read_peak_rss_kib() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
    })
}

fn measure(config: &Config, lut: &Lut) -> Result<Measurement, String> {
    let (available, effective) = effective_workers(config.resident, config.requested_workers);
    let warm = black_box(execute(config, lut, effective)?);
    let mut timings = Vec::with_capacity(config.repeats);
    let mut checksum = None;
    for _ in 0..config.repeats {
        let started = Instant::now();
        let result = black_box(execute(config, lut, effective)?);
        let elapsed = started.elapsed().as_nanos();
        if let Some(expected) = checksum {
            if result != expected {
                return Err("checksum changed between repeated trials".into());
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

fn worker_tile_capacity_particles(resident: usize, workers: usize, tile: usize) -> usize {
    (0..workers).fold(0_usize, |total, worker| {
        let (start, end) = partition(resident, workers, worker);
        total.saturating_add(tile.min(end.saturating_sub(start)))
    })
}

fn json_optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "null".into(), |value| value.to_string())
}

fn receipt_json(config: &Config, lut: &Lut, measurement: Measurement) -> String {
    let compact_bytes = 5 * size_of::<u32>();
    let scratch_bytes = config.reduction.scratch_bytes_per_particle();
    let tile_particles = worker_tile_capacity_particles(
        config.resident,
        measurement.effective_workers,
        config.tile_particles,
    );
    let tile_bytes = (compact_bytes + scratch_bytes).saturating_mul(tile_particles);
    let algorithmic_bytes = tile_bytes.saturating_add(lut.storage_bytes());
    format!(
        "{{\n  \"schema\": \"{RECEIPT_SCHEMA}\",\n  \"runtime\": \"memory-wall-probe\",\n  \"execution_mode\": \"stream-reduce-discard-experiment\",\n  \"reduction_mode\": \"{}\",\n  \"materializes_contribution_array\": {},\n  \"lut_mode\": \"{}\",\n  \"lut_exact_sample_parity\": true,\n  \"lut_storage_bytes\": {},\n  \"lut_correction_palette_entries\": {},\n  \"architecture\": \"{}\",\n  \"os\": \"{}\",\n  \"addressing\": \"{ADDRESSING}\",\n  \"logical_population\": \"{}\",\n  \"resident_particles\": {},\n  \"frames\": {},\n  \"repeats\": {},\n  \"seed\": {},\n  \"requested_workers\": {},\n  \"available_parallelism\": {},\n  \"effective_workers\": {},\n  \"microtile_particles\": {},\n  \"compact_field_bytes_per_particle\": {},\n  \"scratch_bytes_per_particle\": {},\n  \"working_bytes_per_tiled_particle\": {},\n  \"worker_microtile_capacity_bytes\": {},\n  \"algorithmic_working_set_bytes\": {},\n  \"best_ns\": {},\n  \"median_ns\": {},\n  \"checksum\": \"{:016x}\",\n  \"peak_rss_kib\": {},\n  \"resident_generation_in_timed_region\": true,\n  \"timing_scope\": \"resident-generation-plus-bam-lut-projection-plus-hash-plus-wrapping-reduction\",\n  \"claim_boundary\": \"PE #15 memory-wall experiment. Exact checksum and LUT-sample parity are required; timing and Linux VmHWM remain host/configuration-specific evidence. No production promotion is implied.\"\n}}\n",
        config.reduction.name(),
        config.reduction.materializes_contributions(),
        config.lut_mode.name(),
        lut.storage_bytes(),
        lut.correction_palette_entries(),
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
        compact_bytes,
        scratch_bytes,
        compact_bytes + scratch_bytes,
        tile_bytes,
        algorithmic_bytes,
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
    let lut = Lut::build(config.lut_mode)?;
    let measurement = measure(&config, &lut)?;
    let compact_bytes = 5 * size_of::<u32>();
    let scratch_bytes = config.reduction.scratch_bytes_per_particle();
    let capacity_particles = worker_tile_capacity_particles(
        config.resident,
        measurement.effective_workers,
        config.tile_particles,
    );
    let tile_bytes = (compact_bytes + scratch_bytes).saturating_mul(capacity_particles);
    println!("galaxy_cpu_memory_wall=v1");
    println!("reduction_mode={}", config.reduction.name());
    println!(
        "materializes_contribution_array={}",
        config.reduction.materializes_contributions()
    );
    println!("lut_mode={}", config.lut_mode.name());
    println!("lut_storage_bytes={}", lut.storage_bytes());
    println!("logical_population={}", config.logical);
    println!("resident_particles={}", config.resident);
    println!("frames={}", config.frames);
    println!("requested_workers={}", config.requested_workers);
    println!("effective_workers={}", measurement.effective_workers);
    println!("microtile_particles={}", config.tile_particles);
    println!(
        "working_bytes_per_tiled_particle={}",
        compact_bytes + scratch_bytes
    );
    println!("worker_microtile_capacity_bytes={tile_bytes}");
    println!(
        "algorithmic_working_set_bytes={}",
        tile_bytes + lut.storage_bytes()
    );
    println!("best_ns={}", measurement.timing.best_ns);
    println!("median_ns={}", measurement.timing.median_ns);
    println!("checksum={:016x}", measurement.timing.checksum);
    match measurement.peak_rss_kib {
        Some(value) => println!("peak_rss_kib={value}"),
        None => println!("peak_rss_kib=unavailable"),
    }
    if let Some(path) = &config.receipt {
        write_receipt(path, &receipt_json(&config, &lut, measurement))?;
        println!("receipt={}", path.display());
    }
    Ok(())
}

fn parse_verify_workers(args: &[String]) -> Result<usize, String> {
    if args.is_empty() {
        return Ok(default_requested_workers());
    }
    if args.len() != 2 || args[0] != "--workers" {
        return Err("verify accepts only optional --workers N".into());
    }
    parse_positive("--workers", args[1].clone(), MAX_WORKERS)
}

fn run_verify(requested_workers: usize) -> Result<(), String> {
    let full_lut = Lut::build(LutMode::Full)?;
    let cosine_lut = Lut::build(LutMode::CosineCorrected)?;
    let quarter_lut = Lut::build(LutMode::QuarterCorrected)?;

    if full_lut.storage_bytes() != 131_072 {
        return Err("full LUT storage changed unexpectedly".into());
    }
    if cosine_lut.storage_bytes() >= full_lut.storage_bytes() {
        return Err("cosine-corrected LUT did not reduce storage".into());
    }
    if quarter_lut.storage_bytes() >= cosine_lut.storage_bytes() {
        return Err("quarter-corrected LUT did not reduce storage below cosine mode".into());
    }

    for index in 0..LUT_SIZE {
        if cosine_lut.sample(index) != full_lut.sample(index)
            || quarter_lut.sample(index) != full_lut.sample(index)
        {
            return Err(format!("compressed LUT sample mismatch at {index}"));
        }
    }
    for angle in [
        0_u32,
        1,
        0x0003_ffff,
        0x1234_5678,
        0x3fff_ffff,
        0x4000_0000,
        0x7fff_ffff,
        0x8000_0000,
        0xffff_ffff,
    ] {
        let expected = full_lut.sin_cos(angle);
        if cosine_lut.sin_cos(angle) != expected || quarter_lut.sin_cos(angle) != expected {
            return Err(format!("compressed LUT interpolation mismatch at {angle:#010x}"));
        }
    }

    let mut config = Config {
        logical: u64::MAX,
        resident: 8_192,
        frames: 3,
        requested_workers,
        tile_particles: 128,
        repeats: 1,
        seed: DEFAULT_SEED,
        reduction: ReductionMode::Fused,
        lut_mode: LutMode::Full,
        receipt: None,
    };
    let (_, effective) = effective_workers(config.resident, requested_workers);
    let oracle = streaming_reference_checksum(&config, &full_lut)?;

    for (lut_mode, lut) in [
        (LutMode::Full, &full_lut),
        (LutMode::CosineCorrected, &cosine_lut),
        (LutMode::QuarterCorrected, &quarter_lut),
    ] {
        for reduction in [ReductionMode::Materialized, ReductionMode::Fused] {
            for tile in MICRO_TILES {
                config.lut_mode = lut_mode;
                config.reduction = reduction;
                config.tile_particles = tile;
                let value = execute(&config, lut, effective)?;
                if value != oracle {
                    return Err(format!(
                        "parity mismatch lut={} reduction={} tile={tile}: {value:016x} != {oracle:016x}",
                        lut_mode.name(),
                        reduction.name()
                    ));
                }
            }
        }
    }

    let ids = [0_u64, 1, 1_u64 << 32, u64::MAX - 1];
    let id_lo: Vec<u32> = ids.iter().map(|id| *id as u32).collect();
    let id_hi: Vec<u32> = ids.iter().map(|id| (*id >> 32) as u32).collect();
    let x: Vec<u32> = (0..ids.len())
        .map(|index| hash32(index as u32 ^ 0x1234_5678))
        .collect();
    let y: Vec<u32> = (0..ids.len())
        .map(|index| hash32(index as u32 ^ 0x9abc_def0))
        .collect();
    let mut out = vec![0_u64; ids.len()];
    galaxy_memory_hash_batch(&id_lo, &id_hi, &x, &y, &mut out);
    let expected = out
        .iter()
        .fold(0_u64, |sum, value| sum.wrapping_add(*value));
    if galaxy_memory_hash_reduce(&id_lo, &id_hi, &x, &y) != expected {
        return Err("fused hash/reduce diverged from materialized batch reduction".into());
    }

    println!("verify_checksum={oracle:016x}");
    println!("full_lut_storage_bytes={}", full_lut.storage_bytes());
    println!("cosine_lut_storage_bytes={}", cosine_lut.storage_bytes());
    println!("quarter_lut_storage_bytes={}", quarter_lut.storage_bytes());
    println!(
        "quarter_correction_palette_entries={}",
        quarter_lut.correction_palette_entries()
    );
    println!("microtiles=1024,512,256,128,64");
    println!("GALAXY PE15 memory-wall verification passed");
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
        Some("verify") => parse_verify_workers(&args[1..]).and_then(run_verify),
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
        eprintln!("memory_wall_probe: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_luts_are_exact_and_smaller() {
        let full = Lut::build(LutMode::Full).unwrap();
        let cosine = Lut::build(LutMode::CosineCorrected).unwrap();
        let quarter = Lut::build(LutMode::QuarterCorrected).unwrap();
        assert_eq!(full.storage_bytes(), 131_072);
        assert!(cosine.storage_bytes() < full.storage_bytes());
        assert!(quarter.storage_bytes() < cosine.storage_bytes());
        for index in 0..LUT_SIZE {
            assert_eq!(cosine.sample(index), full.sample(index));
            assert_eq!(quarter.sample(index), full.sample(index));
        }
    }

    #[test]
    fn fused_reduction_matches_materialized_reduction() {
        let ids = [0_u64, 1, 1_u64 << 32, u64::MAX - 1];
        let id_lo: Vec<u32> = ids.iter().map(|id| *id as u32).collect();
        let id_hi: Vec<u32> = ids.iter().map(|id| (*id >> 32) as u32).collect();
        let x = vec![1, 2, 3, u32::MAX];
        let y = vec![u32::MAX, 3, 2, 1];
        let mut out = vec![0_u64; ids.len()];
        galaxy_memory_hash_batch(&id_lo, &id_hi, &x, &y, &mut out);
        let expected = out
            .iter()
            .fold(0_u64, |sum, value| sum.wrapping_add(*value));
        assert_eq!(
            galaxy_memory_hash_reduce(&id_lo, &id_hi, &x, &y),
            expected
        );
    }

    #[test]
    fn fused_mode_removes_eight_scratch_bytes_per_particle() {
        assert_eq!(ReductionMode::Materialized.scratch_bytes_per_particle(), 16);
        assert_eq!(ReductionMode::Fused.scratch_bytes_per_particle(), 8);
    }

    #[test]
    fn partition_covers_population() {
        let total = 101;
        let workers = 7;
        let mut cursor = 0;
        for worker in 0..workers {
            let (start, end) = partition(total, workers, worker);
            assert_eq!(start, cursor);
            cursor = end;
        }
        assert_eq!(cursor, total);
    }
}
