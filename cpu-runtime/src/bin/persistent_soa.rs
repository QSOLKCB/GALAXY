// SPDX-License-Identifier: Apache-2.0
//! Persistent worker-pool execution for the guarded production SoA path.
//!
//! This module is included as a child of `integrated_soa`, so it reuses the
//! exact PR #11/#12 particle generation, BAM-LUT projection, contribution hash,
//! partitioning, and deterministic wrapping-u64 reduction contract.

use std::{
    collections::BTreeSet,
    sync::{mpsc, Arc},
    thread::JoinHandle,
};

const POOLED_RECEIPT_SCHEMA: &str = "galaxy.cpu-runtime-persistent-soa-receipt.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchedulePolicy {
    PhysicalFirst,
    Logical,
}

impl SchedulePolicy {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "physical-first" | "physical" => Ok(Self::PhysicalFirst),
            "logical" | "smt" => Ok(Self::Logical),
            _ => Err("--schedule must be physical-first or logical".into()),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::PhysicalFirst => "physical-first",
            Self::Logical => "logical",
        }
    }
}

#[derive(Clone, Debug)]
struct TopologyInfo {
    logical_cpus: usize,
    physical_cores: Option<usize>,
    source: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct PooledEvidence {
    measurement: Measurement,
    pool_startup_ns: u128,
    pool_thread_spawns: usize,
    pool_dispatches: usize,
}

enum PoolCommand {
    Run(u64),
    Shutdown,
}

struct PoolResult {
    generation: u64,
    worker_index: usize,
    result: Result<u64, String>,
}

struct PersistentSoaPool {
    senders: Vec<mpsc::Sender<PoolCommand>>,
    result_rx: mpsc::Receiver<PoolResult>,
    handles: Vec<JoinHandle<()>>,
    worker_count: usize,
}

fn first_touch_compact_tile_buffers(tile: &mut CompactTile, capacity: usize) {
    // Vec::with_capacity reserves address space but does not guarantee that the
    // backing pages have been physically committed. Resize every worker-local
    // vector before the startup acknowledgement so pool_startup_ns includes the
    // same memory commitment/page-fault work that would otherwise be hidden in
    // the first untimed dispatch. Clear keeps the committed capacity reusable.
    tile.id_lo.resize(capacity, 0);
    tile.id_hi.resize(capacity, 0);
    tile.radius_q16.resize(capacity, 0);
    tile.initial_bam.resize(capacity, 0);
    tile.delta_bam.resize(capacity, 0);
    tile.x_word.resize(capacity, 0);
    tile.y_word.resize(capacity, 0);
    tile.contributions.resize(capacity, 0);

    // Keep the initialization observable to the optimizer; this is a deliberate
    // memory-commit boundary, not data needed by the numerical result.
    std::hint::black_box((
        tile.id_lo.as_slice(),
        tile.id_hi.as_slice(),
        tile.radius_q16.as_slice(),
        tile.initial_bam.as_slice(),
        tile.delta_bam.as_slice(),
        tile.x_word.as_slice(),
        tile.y_word.as_slice(),
        tile.contributions.as_slice(),
    ));
    tile.clear();
}

impl PersistentSoaPool {
    fn new(config: Arc<Config>, lut: Arc<Lut>, worker_count: usize) -> Result<Self, String> {
        if worker_count == 0 || worker_count > config.resident || worker_count > MAX_WORKERS {
            return Err("invalid persistent worker count".into());
        }

        let (result_tx, result_rx) = mpsc::channel::<PoolResult>();
        let (ready_tx, ready_rx) = mpsc::channel::<usize>();
        let mut senders = Vec::with_capacity(worker_count);
        let mut handles = Vec::with_capacity(worker_count);

        for worker_index in 0..worker_count {
            let (command_tx, command_rx) = mpsc::channel::<PoolCommand>();
            senders.push(command_tx);
            let worker_result_tx = result_tx.clone();
            let worker_ready_tx = ready_tx.clone();
            let worker_config = Arc::clone(&config);
            let worker_lut = Arc::clone(&lut);
            let (start, end) = partition(config.resident, worker_count, worker_index);
            handles.push(std::thread::spawn(move || {
                let capacity = worker_config
                    .tile_particles
                    .min(end.saturating_sub(start))
                    .max(1);
                let mut tile = CompactTile::new(capacity);
                first_touch_compact_tile_buffers(&mut tile, capacity);
                if worker_ready_tx.send(worker_index).is_err() {
                    return;
                }
                while let Ok(command) = command_rx.recv() {
                    match command {
                        PoolCommand::Run(generation) => {
                            let result = execute_soa_worker_reused(
                                &mut tile,
                                start,
                                end,
                                &worker_config,
                                &worker_lut,
                            );
                            if worker_result_tx
                                .send(PoolResult {
                                    generation,
                                    worker_index,
                                    result,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        PoolCommand::Shutdown => break,
                    }
                }
            }));
        }
        drop(result_tx);
        drop(ready_tx);

        let mut ready = vec![false; worker_count];
        for _ in 0..worker_count {
            let worker_index = ready_rx
                .recv()
                .map_err(|_| "persistent worker failed during startup".to_string())?;
            if worker_index >= worker_count || ready[worker_index] {
                return Err("invalid persistent worker startup acknowledgement".into());
            }
            ready[worker_index] = true;
        }

        Ok(Self {
            senders,
            result_rx,
            handles,
            worker_count,
        })
    }

    fn execute(&self, generation: u64) -> Result<u64, String> {
        for sender in &self.senders {
            sender
                .send(PoolCommand::Run(generation))
                .map_err(|_| "persistent worker command channel closed".to_string())?;
        }

        let mut results: Vec<Option<Result<u64, String>>> =
            (0..self.worker_count).map(|_| None).collect();
        for _ in 0..self.worker_count {
            let message = self
                .result_rx
                .recv()
                .map_err(|_| "persistent worker result channel closed".to_string())?;
            if message.generation != generation {
                return Err(format!(
                    "persistent worker generation mismatch: {} != {generation}",
                    message.generation
                ));
            }
            if message.worker_index >= self.worker_count
                || results[message.worker_index].is_some()
            {
                return Err("invalid or duplicate persistent worker result".into());
            }
            results[message.worker_index] = Some(message.result);
        }

        // Reduce strictly in worker-index order so completion order cannot affect
        // the deterministic wrapping-u64 checksum contract.
        let mut total = 0_u64;
        for result in results {
            let value = result
                .ok_or_else(|| "missing persistent worker result".to_string())??;
            total = total.wrapping_add(value);
        }
        Ok(total)
    }
}

impl Drop for PersistentSoaPool {
    fn drop(&mut self) {
        for sender in &self.senders {
            let _ = sender.send(PoolCommand::Shutdown);
        }
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

fn execute_soa_worker_reused(
    tile: &mut CompactTile,
    start: usize,
    end: usize,
    config: &Config,
    lut: &Lut,
) -> Result<u64, String> {
    let mut checksum = 0_u64;
    let mut tile_start = start;
    while tile_start < end {
        let tile_end = (tile_start + config.tile_particles).min(end);
        fill_compact_tile(tile, tile_start, tile_end, config)?;
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

fn parse_cpu_list(value: &str) -> Result<Vec<usize>, String> {
    let mut cpus = BTreeSet::new();
    for part in value.trim().split(',').filter(|part| !part.is_empty()) {
        if let Some((first, last)) = part.split_once('-') {
            let first = first
                .trim()
                .parse::<usize>()
                .map_err(|_| "invalid CPU-list range start".to_string())?;
            let last = last
                .trim()
                .parse::<usize>()
                .map_err(|_| "invalid CPU-list range end".to_string())?;
            if first > last {
                return Err("CPU-list range start exceeds end".into());
            }
            cpus.extend(first..=last);
        } else {
            cpus.insert(
                part.trim()
                    .parse::<usize>()
                    .map_err(|_| "invalid CPU-list entry".to_string())?,
            );
        }
    }
    if cpus.is_empty() {
        return Err("CPU list is empty".into());
    }
    Ok(cpus.into_iter().collect())
}

#[cfg(target_os = "linux")]
fn linux_allowed_cpus() -> Option<Vec<usize>> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let value = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))?;
    parse_cpu_list(value).ok()
}

#[cfg(target_os = "linux")]
fn detect_physical_cores(logical_cpus: usize) -> Option<usize> {
    let allowed = linux_allowed_cpus()?;
    let mut cores = BTreeSet::new();
    for cpu in allowed {
        let core_id = std::fs::read_to_string(format!(
            "/sys/devices/system/cpu/cpu{cpu}/topology/core_id"
        ))
        .ok()?
        .trim()
        .parse::<i64>()
        .ok()?;
        let package_id = std::fs::read_to_string(format!(
            "/sys/devices/system/cpu/cpu{cpu}/topology/physical_package_id"
        ))
        .ok()?
        .trim()
        .parse::<i64>()
        .ok()?;
        cores.insert((package_id, core_id));
    }
    (!cores.is_empty()).then_some(cores.len().min(logical_cpus).max(1))
}

#[cfg(target_os = "macos")]
fn detect_physical_cores(logical_cpus: usize) -> Option<usize> {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "hw.physicalcpu"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let parsed = String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse::<usize>()
        .ok()?;
    (parsed > 0).then_some(parsed.min(logical_cpus).max(1))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detect_physical_cores(_logical_cpus: usize) -> Option<usize> {
    None
}

fn detect_topology() -> TopologyInfo {
    let logical_cpus = available_parallelism();
    let physical_cores = detect_physical_cores(logical_cpus);
    let source = if physical_cores.is_some() {
        #[cfg(target_os = "linux")]
        {
            "linux-cpuset-sysfs"
        }
        #[cfg(target_os = "macos")]
        {
            "macos-sysctl"
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            "unavailable"
        }
    } else {
        "unavailable"
    };
    TopologyInfo {
        logical_cpus,
        physical_cores,
        source,
    }
}

fn resolve_pool_workers(
    resident: usize,
    requested_workers: usize,
    topology: &TopologyInfo,
    schedule: SchedulePolicy,
) -> usize {
    let logical_limit = requested_workers
        .min(resident.max(1))
        .min(topology.logical_cpus.max(1))
        .min(MAX_WORKERS)
        .max(1);
    match (schedule, topology.physical_cores) {
        (SchedulePolicy::PhysicalFirst, Some(physical)) => logical_limit.min(physical.max(1)),
        _ => logical_limit,
    }
}

fn filter_schedule_args(
    args: &[String],
    default_tile: usize,
) -> Result<(Vec<String>, SchedulePolicy), String> {
    let mut schedule = SchedulePolicy::PhysicalFirst;
    let mut filtered = Vec::with_capacity(args.len() + 2);
    let mut saw_tile = false;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("{flag} requires a value"))?
            .clone();
        match flag {
            "--schedule" => schedule = SchedulePolicy::parse(&value)?,
            "--path" => {
                return Err(
                    "bench-soa-pool selects persistent worker-local SoA; --path is not accepted"
                        .into(),
                )
            }
            "--tile" => {
                saw_tile = true;
                filtered.push(flag.into());
                filtered.push(value);
            }
            _ => {
                filtered.push(flag.into());
                filtered.push(value);
            }
        }
        index += 2;
    }
    if !saw_tile {
        filtered.push("--tile".into());
        filtered.push(default_tile.to_string());
    }
    Ok((filtered, schedule))
}

fn measure_pooled(
    config: Arc<Config>,
    lut: Arc<Lut>,
    worker_count: usize,
) -> Result<PooledEvidence, String> {
    let startup = std::time::Instant::now();
    let pool = PersistentSoaPool::new(config.clone(), lut, worker_count)?;
    let pool_startup_ns = startup.elapsed().as_nanos();

    let warm = std::hint::black_box(pool.execute(0)?);
    let mut timings = Vec::with_capacity(config.repeats);
    let mut checksum = None;
    for repeat in 0..config.repeats {
        let started = std::time::Instant::now();
        let result = std::hint::black_box(pool.execute((repeat + 1) as u64)?);
        let elapsed = started.elapsed().as_nanos();
        if let Some(expected) = checksum {
            if result != expected {
                return Err("pooled checksum changed between repeated trials".into());
            }
        } else {
            checksum = Some(result);
        }
        if result != warm {
            return Err("pooled warm-up and measured checksum disagree".into());
        }
        timings.push(elapsed);
    }
    timings.sort_unstable();
    Ok(PooledEvidence {
        measurement: Measurement {
            timing: Timing {
                best_ns: timings[0],
                median_ns: median_sorted(&timings),
                checksum: checksum.expect("positive repeat count"),
            },
            available_parallelism: available_parallelism(),
            effective_workers: worker_count,
            peak_rss_kib: read_peak_rss_kib(),
        },
        pool_startup_ns,
        pool_thread_spawns: worker_count,
        pool_dispatches: config.repeats + 1,
    })
}

fn json_optional_usize(value: Option<usize>) -> String {
    value.map_or_else(|| "null".into(), |value| value.to_string())
}

fn json_bool_local(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn pooled_receipt_json(
    config: &Config,
    topology: &TopologyInfo,
    schedule: SchedulePolicy,
    evidence: PooledEvidence,
) -> String {
    let compact_field_bytes = 5 * std::mem::size_of::<u32>();
    let scratch_bytes = 2 * std::mem::size_of::<u32>() + std::mem::size_of::<u64>();
    let worker_tile_capacity_particles = soa_worker_tile_capacity_particles(
        config.resident,
        evidence.measurement.effective_workers,
        config.tile_particles,
    );
    let worker_tile_capacity_bytes = (compact_field_bytes + scratch_bytes)
        .saturating_mul(worker_tile_capacity_particles);
    let fell_back = schedule == SchedulePolicy::PhysicalFirst && topology.physical_cores.is_none();
    format!(
        "{{\n  \"schema\": \"{POOLED_RECEIPT_SCHEMA}\",\n  \"runtime\": \"galaxy-cpu\",\n  \"execution_mode\": \"worker-local-soa-persistent\",\n  \"guarded_opt_in\": true,\n  \"canonical_oracle\": \"bench\",\n  \"spawned_soa_fallback\": \"bench-soa\",\n  \"architecture\": \"{}\",\n  \"os\": \"{}\",\n  \"addressing\": \"{ADDRESSING}\",\n  \"logical_population\": \"{}\",\n  \"resident_particles\": {},\n  \"frames\": {},\n  \"repeats\": {},\n  \"seed\": {},\n  \"requested_workers\": {},\n  \"available_parallelism\": {},\n  \"detected_physical_cores\": {},\n  \"topology_source\": \"{}\",\n  \"schedule_policy\": \"{}\",\n  \"schedule_fell_back_to_logical\": {},\n  \"effective_pool_workers\": {},\n  \"tile_particles\": {},\n  \"soa_worker_tile_capacity_bytes\": {},\n  \"pool_thread_spawns\": {},\n  \"pool_dispatches\": {},\n  \"pool_startup_ns\": {},\n  \"worker_threads_persistent_across_trials\": true,\n  \"worker_thread_creation_in_timed_region\": false,\n  \"worker_local_tile_buffers_reused\": true,\n  \"worker_local_tile_buffers_first_touched_before_ready\": true,\n  \"best_ns\": {},\n  \"median_ns\": {},\n  \"checksum\": \"{:016x}\",\n  \"peak_rss_kib\": {},\n  \"resident_generation_in_timed_region\": true,\n  \"timing_scope\": \"persistent-pool-dispatch-plus-resident-generation-plus-bam-lut-projection-plus-contribution-plus-deterministic-worker-reduction\",\n  \"claim_boundary\": \"Guarded opt-in persistent worker-pool evidence. Pool startup includes worker creation, worker-local tile allocation, and explicit buffer first-touch before readiness. Physical-core detection is best-effort and physical-first falls back to logical scheduling when unavailable. Performance is host-specific; checksum disagreement is a correctness failure.\"\n}}\n",
        std::env::consts::ARCH,
        std::env::consts::OS,
        config.logical,
        config.resident,
        config.frames,
        config.repeats,
        config.seed,
        config.requested_workers,
        topology.logical_cpus,
        json_optional_usize(topology.physical_cores),
        topology.source,
        schedule.name(),
        json_bool_local(fell_back),
        evidence.measurement.effective_workers,
        config.tile_particles,
        worker_tile_capacity_bytes,
        evidence.pool_thread_spawns,
        evidence.pool_dispatches,
        evidence.pool_startup_ns,
        evidence.measurement.timing.best_ns,
        evidence.measurement.timing.median_ns,
        evidence.measurement.timing.checksum,
        json_optional_u64(evidence.measurement.peak_rss_kib),
    )
}

pub fn usage_text() -> &'static str {
    "Usage:\n  galaxy-cpu verify-soa-pool [--workers N] [--tile N] [--schedule physical-first|logical]\n  galaxy-cpu bench-soa-pool [--logical U64] [--resident N] [--frames N] [--workers N] [--tile N] [--schedule physical-first|logical] [--repeats N] [--seed U32] [--receipt PATH]\n\nPersistent SoA defaults:\n  tile=1024 schedule=physical-first\n\n`physical-first` caps the persistent pool at the detected physical-core count when reliable topology is available; otherwise it records the fallback and uses logical availability. `logical` permits SMT workers explicitly. Pool startup includes worker-local tile allocation and explicit buffer first-touch before workers report ready.\n"
}

pub fn run_pooled_bench(args: &[String]) -> Result<(), String> {
    let (filtered, schedule) = filter_schedule_args(args, INTEGRATED_DEFAULT_TILE)?;
    let mut config = parse_config(&filtered)?;
    config.path = ExecutionPath::WorkerSoa;
    let topology = detect_topology();
    let worker_count = resolve_pool_workers(
        config.resident,
        config.requested_workers,
        &topology,
        schedule,
    );
    let lut = Arc::new(Lut::build());
    let evidence = measure_pooled(Arc::new(config.clone()), lut, worker_count)?;

    println!("galaxy_cpu_runtime=v3");
    println!("execution_mode=worker-local-soa-persistent");
    println!("schedule_policy={}", schedule.name());
    println!("topology_source={}", topology.source);
    println!("available_parallelism={}", topology.logical_cpus);
    match topology.physical_cores {
        Some(value) => println!("detected_physical_cores={value}"),
        None => println!("detected_physical_cores=unavailable"),
    }
    println!("requested_workers={}", config.requested_workers);
    println!("effective_pool_workers={worker_count}");
    println!("tile_particles={}", config.tile_particles);
    println!("pool_thread_spawns={}", evidence.pool_thread_spawns);
    println!("pool_dispatches={}", evidence.pool_dispatches);
    println!("pool_startup_ns={}", evidence.pool_startup_ns);
    println!("worker_local_tile_buffers_first_touched_before_ready=true");
    println!("best_ns={}", evidence.measurement.timing.best_ns);
    println!("median_ns={}", evidence.measurement.timing.median_ns);
    println!("checksum={:016x}", evidence.measurement.timing.checksum);
    match evidence.measurement.peak_rss_kib {
        Some(value) => println!("peak_rss_kib={value}"),
        None => println!("peak_rss_kib=unavailable"),
    }
    if let Some(path) = &config.receipt {
        write_receipt(path, &pooled_receipt_json(&config, &topology, schedule, evidence))?;
        println!("receipt={}", path.display());
    }
    Ok(())
}

pub fn run_pooled_verify(args: &[String]) -> Result<(), String> {
    let (filtered, schedule) = filter_schedule_args(args, INTEGRATED_DEFAULT_TILE)?;
    let (requested_workers, tile) = parse_verify(&filtered)?;

    // Preserve the full PR #11/#12 parity suite first.
    run_verify(requested_workers, tile)?;

    let topology = detect_topology();
    let config = Config {
        path: ExecutionPath::WorkerSoa,
        logical: u64::MAX,
        resident: 32_768,
        frames: 3,
        requested_workers,
        tile_particles: tile,
        repeats: 2,
        seed: DEFAULT_SEED,
        receipt: None,
    };
    let worker_count = resolve_pool_workers(config.resident, requested_workers, &topology, schedule);
    let lut = Arc::new(Lut::build());
    let reference_particles = build_reference_particles(&config)?;
    let reference = execute_reference_range(&reference_particles, config.frames, &lut);
    let spawned = execute_soa(&config, &lut, worker_count)?;
    if spawned != reference {
        return Err(format!(
            "spawned SoA/reference checksum mismatch: {spawned:016x} != {reference:016x}"
        ));
    }

    let pool = PersistentSoaPool::new(Arc::new(config), Arc::clone(&lut), worker_count)?;
    let first = pool.execute(1)?;
    let second = pool.execute(2)?;
    if first != reference || second != reference {
        return Err(format!(
            "persistent SoA checksum mismatch: first={first:016x} second={second:016x} reference={reference:016x}"
        ));
    }

    println!("verify_persistent_checksum={reference:016x}");
    println!("verify_schedule_policy={}", schedule.name());
    println!("verify_topology_source={}", topology.source);
    println!("verify_available_parallelism={}", topology.logical_cpus);
    match topology.physical_cores {
        Some(value) => println!("verify_detected_physical_cores={value}"),
        None => println!("verify_detected_physical_cores=unavailable"),
    }
    println!("verify_effective_pool_workers={worker_count}");
    println!("GALAXY persistent worker-local SoA verification passed");
    Ok(())
}

#[cfg(test)]
mod pooled_tests {
    use super::*;

    #[test]
    fn cpu_list_parser_handles_ranges_and_singletons() {
        assert_eq!(parse_cpu_list("0-3,8,10-11").unwrap(), vec![0, 1, 2, 3, 8, 10, 11]);
        assert!(parse_cpu_list("3-1").is_err());
        assert!(parse_cpu_list("").is_err());
    }

    #[test]
    fn first_touch_primes_full_capacity_and_preserves_reuse_shape() {
        let capacity = 257;
        let mut tile = CompactTile::new(capacity);
        first_touch_compact_tile_buffers(&mut tile, capacity);
        assert!(tile.id_lo.is_empty());
        assert!(tile.id_hi.is_empty());
        assert!(tile.radius_q16.is_empty());
        assert!(tile.initial_bam.is_empty());
        assert!(tile.delta_bam.is_empty());
        assert!(tile.x_word.is_empty());
        assert!(tile.y_word.is_empty());
        assert!(tile.contributions.is_empty());
        assert!(tile.id_lo.capacity() >= capacity);
        assert!(tile.id_hi.capacity() >= capacity);
        assert!(tile.radius_q16.capacity() >= capacity);
        assert!(tile.initial_bam.capacity() >= capacity);
        assert!(tile.delta_bam.capacity() >= capacity);
        assert!(tile.x_word.capacity() >= capacity);
        assert!(tile.y_word.capacity() >= capacity);
        assert!(tile.contributions.capacity() >= capacity);
    }

    #[test]
    fn physical_first_caps_workers_when_topology_is_known() {
        let topology = TopologyInfo {
            logical_cpus: 32,
            physical_cores: Some(16),
            source: "test",
        };
        assert_eq!(
            resolve_pool_workers(1_000, 32, &topology, SchedulePolicy::PhysicalFirst),
            16
        );
        assert_eq!(
            resolve_pool_workers(1_000, 32, &topology, SchedulePolicy::Logical),
            32
        );
    }

    #[test]
    fn physical_first_falls_back_when_topology_is_unknown() {
        let topology = TopologyInfo {
            logical_cpus: 12,
            physical_cores: None,
            source: "unavailable",
        };
        assert_eq!(
            resolve_pool_workers(1_000, 24, &topology, SchedulePolicy::PhysicalFirst),
            12
        );
    }
}
