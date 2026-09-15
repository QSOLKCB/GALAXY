// SPDX-License-Identifier: Apache-2.0
//! PE #14 calibrated host-aware promotion policy for GALAXY CPU execution.
//!
//! This module is included beneath the persistent SoA module, so it reuses the
//! exact canonical/reference, spawned-SoA, persistent-pool, topology, LUT,
//! and checksum primitives already validated by PE #11-#13.

const AUTO_RECEIPT_SCHEMA: &str = "galaxy.cpu-runtime-auto-receipt.v1";
const AUTO_POLICY: &str = "calibrated-host-auto-v1";
const AUTO_ORACLE: &str = "streaming-canonical-bam-lut-v1";
const AUTO_SCORE_PROJECTION: &str = "linear-particle-frame-v1";
const AUTO_TILE_SHAPE_POLICY: &str = "expand-resident-for-effective-tile-v1";
const AUTO_FRAME_POLICY: &str = "preserve-requested-depth-v1";
const AUTO_RSS_SCOPE: &str = "whole-auto-invocation";
const AUTO_RSS_METRIC: &str = "linux-vmhwm-process-high-water-mark-when-available";
const AUTO_CALIBRATION_BASE_RESIDENT: usize = 65_536;
const AUTO_CALIBRATION_REPEATS: usize = 3;
const AUTO_PROMOTION_MARGIN_BPS: u128 = 500; // 5% required before leaving canonical.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutoEngine {
    Canonical,
    SpawnedSoa,
    PersistentPhysical,
    PersistentLogical,
}

impl AutoEngine {
    fn name(self) -> &'static str {
        match self {
            Self::Canonical => "canonical",
            Self::SpawnedSoa => "spawned-soa",
            Self::PersistentPhysical => "persistent-physical-first",
            Self::PersistentLogical => "persistent-logical",
        }
    }

    fn tie_rank(self) -> u8 {
        match self {
            Self::Canonical => 0,
            Self::SpawnedSoa => 1,
            Self::PersistentPhysical => 2,
            Self::PersistentLogical => 3,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AutoCandidate {
    engine: AutoEngine,
    tile_particles: Option<usize>,
    calibration_workers: usize,
    effective_workers: usize,
    calibration_effective_tile_particles: Option<usize>,
    requested_effective_tile_particles: Option<usize>,
    requested_schedule: Option<SchedulePolicy>,
    effective_schedule: Option<SchedulePolicy>,
    schedule_fell_back_to_logical: bool,
    calibration_median_ns: u128,
    projected_median_ns: u128,
    calibration_startup_ns: u128,
    // Persistent candidates use an observed startup for the full requested pool.
    // Non-persistent candidates keep this at zero because their normal setup is
    // already included in the measured execution median projected below.
    startup_ns: u128,
    score_ns: u128,
    checksum: u64,
}

#[derive(Clone, Copy, Debug)]
struct AutoRunEvidence {
    measurement: Measurement,
    pool_startup_ns: Option<u128>,
}

pub fn auto_usage_text() -> &'static str {
    "Usage:\n  galaxy-cpu verify-auto [--workers N]\n  galaxy-cpu bench-auto [--logical U64] [--resident N] [--frames N] [--workers N] [--repeats N] [--seed U32] [--receipt PATH]\n\nPE #14 auto policy:\n  - calibrates canonical, spawned SoA, and persistent physical/logical candidates\n  - tunes over the evidence-backed tile set {1024,4096,16384,65536}\n  - starts from a 65536-particle calibration base and expands resident work when needed so every advertised tile is measured at the same effective per-worker size it will have on the requested workload\n  - preserves the requested frame depth during calibration so per-particle setup versus per-frame work keeps the requested cost mix\n  - projects every calibration median to requested particle-frame work\n  - includes topology detection in tuning_ns\n  - scores persistent candidates with observed full-requested-pool startup, including worker-local buffer first-touch\n  - reports Linux VmHWM only as the whole auto invocation high-water mark; an isolated selected-run peak is not available in-process\n  - requires at least a 5% projected win before promoting away from canonical\n  - verifies every calibration candidate against an independent streaming canonical BAM-LUT oracle\n  - verifies the selected full-workload result against the same full-workload oracle\n\nManual control remains available through `bench`, `bench-soa`, and `bench-soa-pool`. `bench-auto` therefore rejects --path, --tile, and --schedule.\n"
}

fn filter_auto_args(args: &[String]) -> Result<Vec<String>, String> {
    let mut filtered = Vec::with_capacity(args.len() + 2);
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("{flag} requires a value"))?
            .clone();
        match flag {
            "--path" | "--tile" | "--schedule" => {
                return Err(format!(
                    "bench-auto owns runtime/tile/schedule selection; {flag} is not accepted; use bench, bench-soa, or bench-soa-pool for manual control"
                ));
            }
            _ => {
                filtered.push(flag.into());
                filtered.push(value);
            }
        }
        index += 2;
    }
    // parse_config requires a numeric tile even though auto replaces it.
    filtered.push("--tile".into());
    filtered.push(INTEGRATED_DEFAULT_TILE.to_string());
    Ok(filtered)
}

fn parse_auto_verify_workers(args: &[String]) -> Result<usize, String> {
    let mut workers = default_requested_workers();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        if flag != "--workers" {
            return Err(format!("verify-auto only accepts --workers N; got {flag}"));
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| "--workers requires a value".to_string())?
            .clone();
        workers = parse_positive("--workers", value, MAX_WORKERS)?;
        index += 2;
    }
    Ok(workers)
}

fn auto_tiles(resident: usize) -> Vec<usize> {
    let mut tiles = vec![1_024_usize, 4_096, 16_384, 65_536];
    for tile in &mut tiles {
        *tile = (*tile).min(resident.max(1));
    }
    tiles.sort_unstable();
    tiles.dedup();
    tiles
}

fn minimum_partition_len(resident: usize, worker_count: usize) -> usize {
    resident / worker_count.max(1)
}

fn effective_tile_particles(resident: usize, worker_count: usize, tile: usize) -> usize {
    tile.min(minimum_partition_len(resident, worker_count).max(1))
}

fn tile_faithful_calibration_resident(full: &Config, topology: &TopologyInfo) -> usize {
    let base = full.resident.min(AUTO_CALIBRATION_BASE_RESIDENT).max(1);
    let logical_workers = resolve_pool_workers(
        full.resident,
        full.requested_workers,
        topology,
        SchedulePolicy::Logical,
    );
    let max_tile = auto_tiles(full.resident).into_iter().max().unwrap_or(1);
    let full_min_partition = minimum_partition_len(full.resident, logical_workers).max(1);

    // If every requested logical worker can consume the largest candidate tile,
    // grow only far enough to make that tile real during calibration. Otherwise
    // the requested workload itself is partition-limited, so use its exact
    // resident shape rather than pretending a smaller calibration has the same
    // per-worker cache/tile behavior.
    let required = if max_tile <= full_min_partition {
        max_tile.saturating_mul(logical_workers).min(full.resident)
    } else {
        full.resident
    };
    base.max(required).min(full.resident).max(1)
}

fn calibration_config(full: &Config, topology: &TopologyInfo) -> Config {
    let mut config = full.clone();
    config.resident = tile_faithful_calibration_resident(full, topology);
    // Preserve the requested frame depth. Particle generation/allocation happens
    // outside the frame loop while projection/hash work scales with frames, so a
    // shallow fixed frame cap can change candidate ordering for deep workloads.
    config.frames = full.frames.max(1);
    config.repeats = AUTO_CALIBRATION_REPEATS.min(MAX_REPEATS).max(1);
    config.receipt = None;
    config
}

fn workload_units(config: &Config) -> u128 {
    (config.resident as u128)
        .saturating_mul(config.frames as u128)
        .max(1)
}

fn project_median_ns(calibration_median_ns: u128, calibration: &Config, full: &Config) -> u128 {
    let calibration_units = workload_units(calibration);
    let requested_units = workload_units(full);
    let numerator = calibration_median_ns.saturating_mul(requested_units);
    numerator
        .saturating_add(calibration_units.saturating_sub(1))
        / calibration_units
}

fn streaming_oracle_checksum(config: &Config, lut: &Lut) -> Result<u64, String> {
    let mut checksum = 0_u64;
    for index in 0..config.resident {
        let (id, radius_q16, initial_bam, delta_bam, _, _) =
            particle_fields(index, config.resident, config.logical, config.seed)?;
        for frame in 0..config.frames {
            let angle = initial_bam.wrapping_add(delta_bam.wrapping_mul(frame as u32));
            let (cos, sin) = lut.sin_cos(angle);
            let radius = radius_q16 as i64;
            let x = (radius * cos) >> 30;
            let y = (radius * sin) >> 30;
            checksum = checksum.wrapping_add(contribution_reference(id, x, y));
        }
    }
    Ok(checksum)
}

fn candidate_score(projected_median_ns: u128, startup_ns: u128, amortization_repeats: usize) -> u128 {
    projected_median_ns.saturating_add(startup_ns / amortization_repeats.max(1) as u128)
}

fn effective_schedule(requested: SchedulePolicy, topology: &TopologyInfo) -> SchedulePolicy {
    if requested == SchedulePolicy::PhysicalFirst && topology.physical_cores.is_none() {
        SchedulePolicy::Logical
    } else {
        requested
    }
}

fn schedule_fell_back_to_logical(requested: SchedulePolicy, topology: &TopologyInfo) -> bool {
    requested == SchedulePolicy::PhysicalFirst && topology.physical_cores.is_none()
}

fn engine_for_schedule(schedule: SchedulePolicy) -> AutoEngine {
    match schedule {
        SchedulePolicy::PhysicalFirst => AutoEngine::PersistentPhysical,
        SchedulePolicy::Logical => AutoEngine::PersistentLogical,
    }
}

fn verify_effective_tile_shape(
    engine: AutoEngine,
    tile: usize,
    calibration_resident: usize,
    calibration_workers: usize,
    requested_resident: usize,
    requested_workers: usize,
) -> Result<(usize, usize), String> {
    let calibration_effective =
        effective_tile_particles(calibration_resident, calibration_workers, tile);
    let requested_effective = effective_tile_particles(requested_resident, requested_workers, tile);
    if calibration_effective != requested_effective {
        return Err(format!(
            "auto calibration tile-shape mismatch for {} tile {tile}: calibration effective tile {calibration_effective} != requested effective tile {requested_effective}",
            engine.name()
        ));
    }
    Ok((calibration_effective, requested_effective))
}

fn calibrate_non_persistent(
    base: &Config,
    full: &Config,
    engine: AutoEngine,
    tile: Option<usize>,
    oracle: u64,
) -> Result<AutoCandidate, String> {
    let mut config = base.clone();
    config.path = match engine {
        AutoEngine::Canonical => ExecutionPath::Reference,
        AutoEngine::SpawnedSoa => ExecutionPath::WorkerSoa,
        _ => return Err("internal auto calibration engine mismatch".into()),
    };
    if let Some(tile) = tile {
        config.tile_particles = tile;
    }
    let lut = Lut::build();
    let measurement = measure(&config, &lut)?;
    if measurement.timing.checksum != oracle {
        return Err(format!(
            "auto calibration fail-closed: {} checksum {:016x} != oracle {oracle:016x}",
            engine.name(), measurement.timing.checksum
        ));
    }

    let full_workers = effective_workers(full.resident, full.requested_workers).1;
    let (calibration_effective_tile_particles, requested_effective_tile_particles) =
        if let Some(tile) = tile {
            let (calibration_effective, requested_effective) = verify_effective_tile_shape(
                engine,
                tile,
                config.resident,
                measurement.effective_workers,
                full.resident,
                full_workers,
            )?;
            (Some(calibration_effective), Some(requested_effective))
        } else {
            (None, None)
        };

    let projected_median_ns = project_median_ns(measurement.timing.median_ns, base, full);
    Ok(AutoCandidate {
        engine,
        tile_particles: tile,
        calibration_workers: measurement.effective_workers,
        effective_workers: full_workers,
        calibration_effective_tile_particles,
        requested_effective_tile_particles,
        requested_schedule: None,
        effective_schedule: None,
        schedule_fell_back_to_logical: false,
        calibration_median_ns: measurement.timing.median_ns,
        projected_median_ns,
        calibration_startup_ns: 0,
        startup_ns: 0,
        score_ns: projected_median_ns,
        checksum: measurement.timing.checksum,
    })
}

fn measure_full_pool_startup(
    full: &Config,
    topology: &TopologyInfo,
    schedule: SchedulePolicy,
    tile: usize,
) -> Result<(usize, u128), String> {
    let mut config = full.clone();
    config.path = ExecutionPath::WorkerSoa;
    config.tile_particles = tile.min(config.resident.max(1));
    config.receipt = None;
    let worker_count = resolve_pool_workers(
        config.resident,
        config.requested_workers,
        topology,
        schedule,
    );

    // Match measure_pooled's startup boundary: LUT construction is setup shared by
    // the engine and intentionally outside the pool-startup timer. Pool::new now
    // includes worker creation, full requested tile allocation, and explicit
    // worker-local buffer first-touch before readiness.
    let lut = Arc::new(Lut::build());
    let started = std::time::Instant::now();
    let pool = PersistentSoaPool::new(Arc::new(config), lut, worker_count)?;
    let startup_ns = started.elapsed().as_nanos();
    drop(pool);
    Ok((worker_count, startup_ns))
}

fn calibrate_persistent(
    base: &Config,
    full: &Config,
    topology: &TopologyInfo,
    requested_schedule: SchedulePolicy,
    tile: usize,
    oracle: u64,
    amortization_repeats: usize,
) -> Result<AutoCandidate, String> {
    let mut config = base.clone();
    config.path = ExecutionPath::WorkerSoa;
    config.tile_particles = tile;
    let calibration_worker_count = resolve_pool_workers(
        config.resident,
        config.requested_workers,
        topology,
        requested_schedule,
    );
    let lut = Arc::new(Lut::build());
    let evidence = measure_pooled(Arc::new(config.clone()), lut, calibration_worker_count)?;
    if evidence.measurement.timing.checksum != oracle {
        return Err(format!(
            "auto calibration fail-closed: persistent {} checksum {:016x} != oracle {oracle:016x}",
            requested_schedule.name(), evidence.measurement.timing.checksum
        ));
    }

    // The calibration resident shape is expanded when necessary so the nominal
    // tile maps to the same effective per-worker tile as the requested workload.
    // Full pool startup is still observed separately because allocation/thread
    // creation/first-touch cost depends on the complete requested execution shape.
    let (full_worker_count, full_pool_startup_ns) =
        measure_full_pool_startup(full, topology, requested_schedule, tile)?;
    let effective_schedule = effective_schedule(requested_schedule, topology);
    let engine = engine_for_schedule(effective_schedule);
    let (calibration_effective_tile_particles, requested_effective_tile_particles) =
        verify_effective_tile_shape(
            engine,
            tile,
            config.resident,
            calibration_worker_count,
            full.resident,
            full_worker_count,
        )?;
    let projected_median_ns =
        project_median_ns(evidence.measurement.timing.median_ns, base, full);

    Ok(AutoCandidate {
        engine,
        tile_particles: Some(tile),
        calibration_workers: calibration_worker_count,
        effective_workers: full_worker_count,
        calibration_effective_tile_particles: Some(calibration_effective_tile_particles),
        requested_effective_tile_particles: Some(requested_effective_tile_particles),
        requested_schedule: Some(requested_schedule),
        effective_schedule: Some(effective_schedule),
        schedule_fell_back_to_logical: schedule_fell_back_to_logical(requested_schedule, topology),
        calibration_median_ns: evidence.measurement.timing.median_ns,
        projected_median_ns,
        calibration_startup_ns: evidence.pool_startup_ns,
        startup_ns: full_pool_startup_ns,
        score_ns: candidate_score(projected_median_ns, full_pool_startup_ns, amortization_repeats),
        checksum: evidence.measurement.timing.checksum,
    })
}

fn choose_candidate(candidates: &[AutoCandidate]) -> Result<AutoCandidate, String> {
    let canonical = candidates
        .iter()
        .copied()
        .find(|candidate| candidate.engine == AutoEngine::Canonical)
        .ok_or_else(|| "auto calibration omitted canonical candidate".to_string())?;
    let optimized = candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.engine != AutoEngine::Canonical)
        .min_by_key(|candidate| {
            (
                candidate.score_ns,
                candidate.engine.tie_rank(),
                candidate.tile_particles.unwrap_or(0),
            )
        });
    let Some(best_optimized) = optimized else {
        return Ok(canonical);
    };

    // Every candidate score is expressed in projected full-requested-workload
    // units before the 5% promotion margin is applied. Persistent candidates then
    // add the observed full-pool startup amortized over requested repeats.
    let lhs = best_optimized.score_ns.saturating_mul(10_000);
    let rhs = canonical
        .score_ns
        .saturating_mul(10_000_u128.saturating_sub(AUTO_PROMOTION_MARGIN_BPS));
    if lhs <= rhs {
        Ok(best_optimized)
    } else {
        Ok(canonical)
    }
}

fn calibrate_auto(
    full: &Config,
    topology: &TopologyInfo,
) -> Result<(Config, Vec<AutoCandidate>, AutoCandidate, u128), String> {
    let calibration = calibration_config(full, topology);
    let oracle_started = std::time::Instant::now();
    let lut = Lut::build();
    let oracle = streaming_oracle_checksum(&calibration, &lut)?;
    let calibration_oracle_ns = oracle_started.elapsed().as_nanos();
    let mut candidates = Vec::new();

    candidates.push(calibrate_non_persistent(
        &calibration,
        full,
        AutoEngine::Canonical,
        None,
        oracle,
    )?);

    let tiles = auto_tiles(full.resident);
    for &tile in &tiles {
        candidates.push(calibrate_non_persistent(
            &calibration,
            full,
            AutoEngine::SpawnedSoa,
            Some(tile),
            oracle,
        )?);
    }

    let physical_workers = resolve_pool_workers(
        full.resident,
        full.requested_workers,
        topology,
        SchedulePolicy::PhysicalFirst,
    );
    let logical_workers = resolve_pool_workers(
        full.resident,
        full.requested_workers,
        topology,
        SchedulePolicy::Logical,
    );

    for &tile in &tiles {
        candidates.push(calibrate_persistent(
            &calibration,
            full,
            topology,
            SchedulePolicy::PhysicalFirst,
            tile,
            oracle,
            full.repeats,
        )?);
    }
    if logical_workers != physical_workers {
        for &tile in &tiles {
            candidates.push(calibrate_persistent(
                &calibration,
                full,
                topology,
                SchedulePolicy::Logical,
                tile,
                oracle,
                full.repeats,
            )?);
        }
    }

    let selected = choose_candidate(&candidates)?;
    Ok((calibration, candidates, selected, calibration_oracle_ns))
}

fn run_selected(
    full: &Config,
    topology: &TopologyInfo,
    selected: AutoCandidate,
) -> Result<AutoRunEvidence, String> {
    match selected.engine {
        AutoEngine::Canonical | AutoEngine::SpawnedSoa => {
            let mut config = full.clone();
            config.path = if selected.engine == AutoEngine::Canonical {
                ExecutionPath::Reference
            } else {
                ExecutionPath::WorkerSoa
            };
            if let Some(tile) = selected.tile_particles {
                config.tile_particles = tile;
            }
            let lut = Lut::build();
            let measurement = measure(&config, &lut)?;
            Ok(AutoRunEvidence {
                measurement,
                pool_startup_ns: None,
            })
        }
        AutoEngine::PersistentPhysical | AutoEngine::PersistentLogical => {
            let mut config = full.clone();
            config.path = ExecutionPath::WorkerSoa;
            config.tile_particles = selected.tile_particles.unwrap_or(INTEGRATED_DEFAULT_TILE);
            let schedule = selected
                .effective_schedule
                .ok_or_else(|| "persistent auto selection omitted effective schedule".to_string())?;
            let worker_count = resolve_pool_workers(
                config.resident,
                config.requested_workers,
                topology,
                schedule,
            );
            let evidence = measure_pooled(
                Arc::new(config),
                Arc::new(Lut::build()),
                worker_count,
            )?;
            Ok(AutoRunEvidence {
                measurement: evidence.measurement,
                pool_startup_ns: Some(evidence.pool_startup_ns),
            })
        }
    }
}

fn optional_tile_json(value: Option<usize>) -> String {
    value.map_or_else(|| "null".into(), |value| value.to_string())
}

fn optional_u128_json(value: Option<u128>) -> String {
    value.map_or_else(|| "null".into(), |value| value.to_string())
}

fn optional_schedule_json(value: Option<SchedulePolicy>) -> String {
    value.map_or_else(|| "null".into(), |schedule| format!("\"{}\"", schedule.name()))
}

fn json_bool_auto(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn candidate_json(candidate: AutoCandidate) -> String {
    let persistent = candidate.effective_schedule.is_some();
    let startup_scope = if persistent {
        "\"full-requested-pool\""
    } else {
        "null"
    };
    format!(
        "{{\"engine\":\"{}\",\"schedule\":{},\"requested_schedule\":{},\"effective_schedule\":{},\"schedule_fell_back_to_logical\":{},\"tile_particles\":{},\"calibration_workers\":{},\"effective_workers\":{},\"calibration_effective_tile_particles\":{},\"requested_effective_tile_particles\":{},\"tile_shape_match\":{},\"calibration_median_ns\":{},\"projected_median_ns\":{},\"calibration_startup_ns\":{},\"startup_ns\":{},\"startup_scope\":{},\"startup_includes_buffer_first_touch\":{},\"score_ns\":{},\"checksum\":\"{:016x}\"}}",
        candidate.engine.name(),
        optional_schedule_json(candidate.effective_schedule),
        optional_schedule_json(candidate.requested_schedule),
        optional_schedule_json(candidate.effective_schedule),
        json_bool_auto(candidate.schedule_fell_back_to_logical),
        optional_tile_json(candidate.tile_particles),
        candidate.calibration_workers,
        candidate.effective_workers,
        optional_tile_json(candidate.calibration_effective_tile_particles),
        optional_tile_json(candidate.requested_effective_tile_particles),
        json_bool_auto(
            candidate.calibration_effective_tile_particles
                == candidate.requested_effective_tile_particles
        ),
        candidate.calibration_median_ns,
        candidate.projected_median_ns,
        candidate.calibration_startup_ns,
        candidate.startup_ns,
        startup_scope,
        json_bool_auto(persistent),
        candidate.score_ns,
        candidate.checksum,
    )
}

fn auto_receipt_json(
    full: &Config,
    topology: &TopologyInfo,
    calibration: &Config,
    candidates: &[AutoCandidate],
    selected: AutoCandidate,
    topology_detection_ns: u128,
    tuning_ns: u128,
    calibration_oracle_ns: u128,
    oracle_ns: u128,
    oracle_checksum: u64,
    evidence: AutoRunEvidence,
) -> String {
    let candidate_blob = candidates
        .iter()
        .copied()
        .map(candidate_json)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\n  \"schema\": \"{AUTO_RECEIPT_SCHEMA}\",\n  \"runtime\": \"galaxy-cpu\",\n  \"execution_mode\": \"host-auto\",\n  \"selection_policy\": \"{AUTO_POLICY}\",\n  \"canonical_oracle\": \"{AUTO_ORACLE}\",\n  \"parity_fail_closed\": true,\n  \"promotion_margin_basis_points\": {AUTO_PROMOTION_MARGIN_BPS},\n  \"score_projection\": \"{AUTO_SCORE_PROJECTION}\",\n  \"tile_shape_policy\": \"{AUTO_TILE_SHAPE_POLICY}\",\n  \"frame_calibration_policy\": \"{AUTO_FRAME_POLICY}\",\n  \"calibration_base_resident_particles\": {AUTO_CALIBRATION_BASE_RESIDENT},\n  \"persistent_startup_score_scope\": \"full-requested-pool\",\n  \"persistent_startup_includes_buffer_first_touch\": true,\n  \"rss_metric\": \"{AUTO_RSS_METRIC}\",\n  \"rss_scope\": \"{AUTO_RSS_SCOPE}\",\n  \"selected_run_peak_rss_available\": false,\n  \"architecture\": \"{}\",\n  \"os\": \"{}\",\n  \"logical_population\": \"{}\",\n  \"resident_particles\": {},\n  \"frames\": {},\n  \"repeats\": {},\n  \"seed\": {},\n  \"requested_workers\": {},\n  \"available_parallelism\": {},\n  \"detected_physical_cores\": {},\n  \"topology_source\": \"{}\",\n  \"topology_detection_ns\": {},\n  \"calibration_resident_particles\": {},\n  \"calibration_frames\": {},\n  \"calibration_repeats\": {},\n  \"calibration_work_units\": {},\n  \"requested_work_units\": {},\n  \"calibration_oracle_ns\": {},\n  \"calibration_candidate_count\": {},\n  \"calibration_candidates\": [{}],\n  \"tuning_ns\": {},\n  \"selected_engine\": \"{}\",\n  \"selected_schedule\": {},\n  \"selected_requested_schedule\": {},\n  \"selected_schedule_fell_back_to_logical\": {},\n  \"selected_tile_particles\": {},\n  \"selected_calibration_effective_tile_particles\": {},\n  \"selected_requested_effective_tile_particles\": {},\n  \"selected_tile_shape_match\": {},\n  \"selected_effective_workers\": {},\n  \"selected_calibration_median_ns\": {},\n  \"selected_projected_median_ns\": {},\n  \"selected_score_ns\": {},\n  \"selected_calibration_pool_startup_ns\": {},\n  \"selected_scored_full_pool_startup_ns\": {},\n  \"selected_pool_startup_ns\": {},\n  \"full_oracle_ns\": {},\n  \"oracle_checksum\": \"{:016x}\",\n  \"selected_checksum\": \"{:016x}\",\n  \"checksum_match\": true,\n  \"best_ns\": {},\n  \"median_ns\": {},\n  \"auto_invocation_peak_rss_kib\": {},\n  \"selected_run_peak_rss_kib\": null,\n  \"claim_boundary\": \"Host-aware selection begins with a bounded deterministic calibration base but expands calibration resident work when needed so each advertised tile is measured at the same effective per-worker tile size as the requested workload. Calibration preserves the requested frame depth so candidate ranking retains the requested per-particle versus per-frame cost mix. Candidate medians are projected to requested particle-frame work before the 5% promotion margin. Persistent candidates additionally include observed full-requested-pool startup, including worker-local buffer first-touch, amortized over requested repeats. Topology detection is included in tuning_ns. Linux VmHWM is process-wide and monotonic, so RSS is reported only as the whole auto invocation high-water mark; this in-process tuner does not claim an isolated selected-run peak RSS. Physical-first fallback is reported explicitly. Every calibration candidate and the selected full-workload result must match the independent streaming canonical BAM-LUT oracle or execution fails closed.\"\n}}\n",
        std::env::consts::ARCH,
        std::env::consts::OS,
        full.logical,
        full.resident,
        full.frames,
        full.repeats,
        full.seed,
        full.requested_workers,
        topology.logical_cpus,
        json_optional_usize(topology.physical_cores),
        topology.source,
        topology_detection_ns,
        calibration.resident,
        calibration.frames,
        calibration.repeats,
        workload_units(calibration),
        workload_units(full),
        calibration_oracle_ns,
        candidates.len(),
        candidate_blob,
        tuning_ns,
        selected.engine.name(),
        optional_schedule_json(selected.effective_schedule),
        optional_schedule_json(selected.requested_schedule),
        json_bool_auto(selected.schedule_fell_back_to_logical),
        optional_tile_json(selected.tile_particles),
        optional_tile_json(selected.calibration_effective_tile_particles),
        optional_tile_json(selected.requested_effective_tile_particles),
        json_bool_auto(
            selected.calibration_effective_tile_particles
                == selected.requested_effective_tile_particles
        ),
        evidence.measurement.effective_workers,
        selected.calibration_median_ns,
        selected.projected_median_ns,
        selected.score_ns,
        selected.calibration_startup_ns,
        selected.startup_ns,
        optional_u128_json(evidence.pool_startup_ns),
        oracle_ns,
        oracle_checksum,
        evidence.measurement.timing.checksum,
        evidence.measurement.timing.best_ns,
        evidence.measurement.timing.median_ns,
        json_optional_u64(evidence.measurement.peak_rss_kib),
    )
}

pub fn run_auto_bench(args: &[String]) -> Result<(), String> {
    let filtered = filter_auto_args(args)?;
    let mut full = parse_config(&filtered)?;
    full.path = ExecutionPath::Reference;

    // Tuning starts before topology detection because topology directly controls
    // the candidate set and effective worker counts.
    let tuning_started = std::time::Instant::now();
    let topology_started = std::time::Instant::now();
    let topology = detect_topology();
    let topology_detection_ns = topology_started.elapsed().as_nanos();

    let (calibration, candidates, selected, calibration_oracle_ns) =
        calibrate_auto(&full, &topology)?;
    let tuning_ns = tuning_started.elapsed().as_nanos();

    let oracle_started = std::time::Instant::now();
    let oracle_lut = Lut::build();
    let oracle_checksum = streaming_oracle_checksum(&full, &oracle_lut)?;
    let oracle_ns = oracle_started.elapsed().as_nanos();

    let evidence = run_selected(&full, &topology, selected)?;
    if evidence.measurement.timing.checksum != oracle_checksum {
        return Err(format!(
            "auto full-workload fail-closed: selected {} checksum {:016x} != oracle {oracle_checksum:016x}",
            selected.engine.name(), evidence.measurement.timing.checksum
        ));
    }

    println!("galaxy_cpu_runtime=v4");
    println!("execution_mode=host-auto");
    println!("selection_policy={AUTO_POLICY}");
    println!("canonical_oracle={AUTO_ORACLE}");
    println!("promotion_margin_basis_points={AUTO_PROMOTION_MARGIN_BPS}");
    println!("score_projection={AUTO_SCORE_PROJECTION}");
    println!("tile_shape_policy={AUTO_TILE_SHAPE_POLICY}");
    println!("frame_calibration_policy={AUTO_FRAME_POLICY}");
    println!("calibration_base_resident_particles={AUTO_CALIBRATION_BASE_RESIDENT}");
    println!("persistent_startup_score_scope=full-requested-pool");
    println!("persistent_startup_includes_buffer_first_touch=true");
    println!("rss_metric={AUTO_RSS_METRIC}");
    println!("rss_scope={AUTO_RSS_SCOPE}");
    println!("selected_run_peak_rss_available=false");
    println!("available_parallelism={}", topology.logical_cpus);
    match topology.physical_cores {
        Some(value) => println!("detected_physical_cores={value}"),
        None => println!("detected_physical_cores=unavailable"),
    }
    println!("topology_source={}", topology.source);
    println!("topology_detection_ns={topology_detection_ns}");
    println!("calibration_resident_particles={}", calibration.resident);
    println!("calibration_frames={}", calibration.frames);
    println!("calibration_work_units={}", workload_units(&calibration));
    println!("requested_work_units={}", workload_units(&full));
    println!("calibration_oracle_ns={calibration_oracle_ns}");
    println!("calibration_candidate_count={}", candidates.len());
    println!("tuning_ns={tuning_ns}");
    println!("selected_engine={}", selected.engine.name());
    match selected.effective_schedule {
        Some(schedule) => println!("selected_schedule={}", schedule.name()),
        None => println!("selected_schedule=none"),
    }
    match selected.requested_schedule {
        Some(schedule) => println!("selected_requested_schedule={}", schedule.name()),
        None => println!("selected_requested_schedule=none"),
    }
    println!(
        "selected_schedule_fell_back_to_logical={}",
        selected.schedule_fell_back_to_logical
    );
    match selected.tile_particles {
        Some(tile) => println!("selected_tile_particles={tile}"),
        None => println!("selected_tile_particles=none"),
    }
    match selected.calibration_effective_tile_particles {
        Some(tile) => println!("selected_calibration_effective_tile_particles={tile}"),
        None => println!("selected_calibration_effective_tile_particles=none"),
    }
    match selected.requested_effective_tile_particles {
        Some(tile) => println!("selected_requested_effective_tile_particles={tile}"),
        None => println!("selected_requested_effective_tile_particles=none"),
    }
    println!(
        "selected_tile_shape_match={}",
        selected.calibration_effective_tile_particles
            == selected.requested_effective_tile_particles
    );
    println!("selected_effective_workers={}", evidence.measurement.effective_workers);
    println!("selected_calibration_median_ns={}", selected.calibration_median_ns);
    println!("selected_projected_median_ns={}", selected.projected_median_ns);
    println!("selected_score_ns={}", selected.score_ns);
    println!(
        "selected_calibration_pool_startup_ns={}",
        selected.calibration_startup_ns
    );
    println!("selected_scored_full_pool_startup_ns={}", selected.startup_ns);
    println!("full_oracle_ns={oracle_ns}");
    println!("checksum={:016x}", evidence.measurement.timing.checksum);
    println!("best_ns={}", evidence.measurement.timing.best_ns);
    println!("median_ns={}", evidence.measurement.timing.median_ns);
    println!("selected_run_peak_rss_kib=unavailable");
    match evidence.measurement.peak_rss_kib {
        Some(value) => println!("auto_invocation_peak_rss_kib={value}"),
        None => println!("auto_invocation_peak_rss_kib=unavailable"),
    }
    match evidence.pool_startup_ns {
        Some(value) => println!("selected_pool_startup_ns={value}"),
        None => println!("selected_pool_startup_ns=none"),
    }
    if let Some(path) = &full.receipt {
        write_receipt(
            path,
            &auto_receipt_json(
                &full,
                &topology,
                &calibration,
                &candidates,
                selected,
                topology_detection_ns,
                tuning_ns,
                calibration_oracle_ns,
                oracle_ns,
                oracle_checksum,
                evidence,
            ),
        )?;
        println!("receipt={}", path.display());
    }
    Ok(())
}

pub fn run_auto_verify(args: &[String]) -> Result<(), String> {
    let requested_workers = parse_auto_verify_workers(args)?;
    let full = Config {
        path: ExecutionPath::Reference,
        logical: u64::MAX,
        resident: 32_768,
        frames: 3,
        requested_workers,
        tile_particles: INTEGRATED_DEFAULT_TILE,
        repeats: 2,
        seed: DEFAULT_SEED,
        receipt: None,
    };
    let topology = detect_topology();
    let (_, candidates, selected, _) = calibrate_auto(&full, &topology)?;
    let oracle = streaming_oracle_checksum(&full, &Lut::build())?;
    let evidence = run_selected(&full, &topology, selected)?;
    if evidence.measurement.timing.checksum != oracle {
        return Err(format!(
            "verify-auto selected checksum {:016x} != oracle {oracle:016x}",
            evidence.measurement.timing.checksum
        ));
    }
    println!("verify_auto_candidates={}", candidates.len());
    println!("verify_auto_selected_engine={}", selected.engine.name());
    println!("verify_auto_checksum={oracle:016x}");
    println!("GALAXY host-auto promotion verification passed");
    Ok(())
}

#[cfg(test)]
mod auto_tests {
    use super::*;

    fn candidate(
        engine: AutoEngine,
        calibration_median_ns: u128,
        projected_median_ns: u128,
        startup_ns: u128,
        score_ns: u128,
    ) -> AutoCandidate {
        let tile = if engine == AutoEngine::Canonical { None } else { Some(1024) };
        AutoCandidate {
            engine,
            tile_particles: tile,
            calibration_workers: 8,
            effective_workers: 8,
            calibration_effective_tile_particles: tile,
            requested_effective_tile_particles: tile,
            requested_schedule: None,
            effective_schedule: None,
            schedule_fell_back_to_logical: false,
            calibration_median_ns,
            projected_median_ns,
            calibration_startup_ns: 0,
            startup_ns,
            score_ns,
            checksum: 1,
        }
    }

    #[test]
    fn projection_scales_particle_frame_work() {
        let calibration = Config {
            path: ExecutionPath::Reference,
            logical: u64::MAX,
            resident: 100,
            frames: 2,
            requested_workers: 2,
            tile_particles: 32,
            repeats: 1,
            seed: DEFAULT_SEED,
            receipt: None,
        };
        let mut full = calibration.clone();
        full.resident = 1_000;
        full.frames = 4;
        assert_eq!(workload_units(&calibration), 200);
        assert_eq!(workload_units(&full), 4_000);
        assert_eq!(project_median_ns(1_000, &calibration, &full), 20_000);
    }

    #[test]
    fn calibration_preserves_requested_frame_depth() {
        let topology = TopologyInfo {
            logical_cpus: 8,
            physical_cores: Some(4),
            source: "test",
        };
        let full = Config {
            path: ExecutionPath::Reference,
            logical: u64::MAX,
            resident: 65_536,
            frames: 64,
            requested_workers: 8,
            tile_particles: INTEGRATED_DEFAULT_TILE,
            repeats: 2,
            seed: DEFAULT_SEED,
            receipt: None,
        };
        let calibration = calibration_config(&full, &topology);
        assert_eq!(calibration.frames, 64);
        assert_eq!(calibration.frames, full.frames);
    }

    #[test]
    fn tile_faithful_calibration_expands_for_large_worker_count() {
        let topology = TopologyInfo {
            logical_cpus: 256,
            physical_cores: Some(128),
            source: "test",
        };
        let full = Config {
            path: ExecutionPath::Reference,
            logical: u64::MAX,
            resident: 16_777_216,
            frames: 8,
            requested_workers: 256,
            tile_particles: INTEGRATED_DEFAULT_TILE,
            repeats: 5,
            seed: DEFAULT_SEED,
            receipt: None,
        };
        let calibration = calibration_config(&full, &topology);
        assert_eq!(calibration.resident, full.resident);
        assert_eq!(calibration.frames, full.frames);
        assert_eq!(
            effective_tile_particles(calibration.resident, 256, 65_536),
            effective_tile_particles(full.resident, 256, 65_536)
        );
        assert_eq!(effective_tile_particles(calibration.resident, 256, 65_536), 65_536);
    }

    #[test]
    fn tile_faithful_calibration_can_stop_below_full_resident() {
        let topology = TopologyInfo {
            logical_cpus: 96,
            physical_cores: Some(48),
            source: "test",
        };
        let full = Config {
            path: ExecutionPath::Reference,
            logical: u64::MAX,
            resident: 16_777_216,
            frames: 8,
            requested_workers: 96,
            tile_particles: INTEGRATED_DEFAULT_TILE,
            repeats: 5,
            seed: DEFAULT_SEED,
            receipt: None,
        };
        let calibration = calibration_config(&full, &topology);
        assert_eq!(calibration.resident, 65_536 * 96);
        assert!(calibration.resident < full.resident);
        assert_eq!(calibration.frames, full.frames);
        assert_eq!(effective_tile_particles(calibration.resident, 96, 65_536), 65_536);
        assert_eq!(effective_tile_particles(full.resident, 96, 65_536), 65_536);
    }

    #[test]
    fn tile_shape_guard_rejects_collapsed_calibration_tile() {
        let mismatch = verify_effective_tile_shape(
            AutoEngine::SpawnedSoa,
            65_536,
            65_536,
            256,
            16_777_216,
            256,
        );
        assert!(mismatch.is_err());
    }

    #[test]
    fn promotion_margin_keeps_near_ties_canonical() {
        let canonical = candidate(AutoEngine::Canonical, 1000, 1000, 0, 1000);
        let near = candidate(AutoEngine::SpawnedSoa, 960, 960, 0, 960);
        assert_eq!(choose_candidate(&[canonical, near]).unwrap().engine, AutoEngine::Canonical);
    }

    #[test]
    fn projected_full_work_can_preserve_persistent_win() {
        let canonical = candidate(AutoEngine::Canonical, 1000, 20_000, 0, 20_000);
        let fast = candidate(
            AutoEngine::PersistentPhysical,
            800,
            16_000,
            300,
            candidate_score(16_000, 300, 1),
        );
        assert_eq!(choose_candidate(&[canonical, fast]).unwrap().engine, AutoEngine::PersistentPhysical);
    }

    #[test]
    fn full_pool_startup_can_block_persistent_promotion() {
        let canonical = candidate(AutoEngine::Canonical, 1000, 1000, 0, 1000);
        let misleading = candidate(
            AutoEngine::PersistentPhysical,
            800,
            800,
            300,
            candidate_score(800, 300, 1),
        );
        assert_eq!(
            choose_candidate(&[canonical, misleading]).unwrap().engine,
            AutoEngine::Canonical
        );
    }

    #[test]
    fn physical_first_fallback_is_reported_as_logical_execution() {
        let topology = TopologyInfo {
            logical_cpus: 12,
            physical_cores: None,
            source: "unavailable",
        };
        assert_eq!(
            effective_schedule(SchedulePolicy::PhysicalFirst, &topology),
            SchedulePolicy::Logical
        );
        assert!(schedule_fell_back_to_logical(
            SchedulePolicy::PhysicalFirst,
            &topology
        ));
    }

    #[test]
    fn streaming_oracle_matches_existing_reference() {
        let config = Config {
            path: ExecutionPath::Reference,
            logical: 1_u64 << 40,
            resident: 2048,
            frames: 3,
            requested_workers: 2,
            tile_particles: 127,
            repeats: 1,
            seed: DEFAULT_SEED,
            receipt: None,
        };
        let lut = Lut::build();
        let particles = build_reference_particles(&config).unwrap();
        let reference = execute_reference_range(&particles, config.frames, &lut);
        let streaming = streaming_oracle_checksum(&config, &lut).unwrap();
        assert_eq!(streaming, reference);
    }
}
