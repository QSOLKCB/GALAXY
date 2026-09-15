// SPDX-License-Identifier: Apache-2.0
//! PE #14 calibrated host-aware promotion policy for GALAXY CPU execution.
//!
//! This module is included beneath the persistent SoA module, so it can reuse
//! the exact canonical/reference, spawned-SoA, persistent-pool, topology, LUT,
//! and checksum primitives already validated by PE #11-#13.

const AUTO_RECEIPT_SCHEMA: &str = "galaxy.cpu-runtime-auto-receipt.v1";
const AUTO_POLICY: &str = "calibrated-host-auto-v1";
const AUTO_ORACLE: &str = "streaming-canonical-bam-lut-v1";
const AUTO_CALIBRATION_MAX_RESIDENT: usize = 65_536;
const AUTO_CALIBRATION_MAX_FRAMES: usize = 4;
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

    fn schedule(self) -> Option<SchedulePolicy> {
        match self {
            Self::PersistentPhysical => Some(SchedulePolicy::PhysicalFirst),
            Self::PersistentLogical => Some(SchedulePolicy::Logical),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AutoCandidate {
    engine: AutoEngine,
    tile_particles: Option<usize>,
    effective_workers: usize,
    median_ns: u128,
    startup_ns: u128,
    score_ns: u128,
    checksum: u64,
}

#[derive(Clone, Copy, Debug)]
struct AutoRunEvidence {
    measurement: Measurement,
    pool_startup_ns: Option<u128>,
}

fn auto_usage_text() -> &'static str {
    "Usage:\n  galaxy-cpu verify-auto [--workers N]\n  galaxy-cpu bench-auto [--logical U64] [--resident N] [--frames N] [--workers N] [--repeats N] [--seed U32] [--receipt PATH]\n\nPE #14 auto policy:\n  - calibrates canonical, spawned SoA, and persistent physical/logical candidates\n  - tunes over the evidence-backed tile set {1024,4096,16384,65536}\n  - requires at least a 5% calibrated win before promoting away from canonical\n  - verifies every calibration candidate against an independent streaming canonical BAM-LUT oracle\n  - verifies the selected full-workload result against the same full-workload oracle\n\nManual control remains available through `bench`, `bench-soa`, and `bench-soa-pool`. `bench-auto` therefore rejects --path, --tile, and --schedule.\n"
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
    // parse_config requires a numeric tile even though auto will replace it.
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

fn calibration_config(full: &Config) -> Config {
    let mut config = full.clone();
    config.resident = full.resident.min(AUTO_CALIBRATION_MAX_RESIDENT).max(1);
    config.frames = full.frames.min(AUTO_CALIBRATION_MAX_FRAMES).max(1);
    config.repeats = AUTO_CALIBRATION_REPEATS.min(MAX_REPEATS).max(1);
    config.receipt = None;
    config
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

fn candidate_score(median_ns: u128, startup_ns: u128, amortization_repeats: usize) -> u128 {
    median_ns.saturating_add(startup_ns / amortization_repeats.max(1) as u128)
}

fn calibrate_non_persistent(
    base: &Config,
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
    Ok(AutoCandidate {
        engine,
        tile_particles: tile,
        effective_workers: measurement.effective_workers,
        median_ns: measurement.timing.median_ns,
        startup_ns: 0,
        score_ns: measurement.timing.median_ns,
        checksum: measurement.timing.checksum,
    })
}

fn calibrate_persistent(
    base: &Config,
    topology: &TopologyInfo,
    schedule: SchedulePolicy,
    tile: usize,
    oracle: u64,
    amortization_repeats: usize,
) -> Result<AutoCandidate, String> {
    let mut config = base.clone();
    config.path = ExecutionPath::WorkerSoa;
    config.tile_particles = tile;
    let worker_count = resolve_pool_workers(
        config.resident,
        config.requested_workers,
        topology,
        schedule,
    );
    let lut = Arc::new(Lut::build());
    let evidence = measure_pooled(Arc::new(config), lut, worker_count)?;
    if evidence.measurement.timing.checksum != oracle {
        return Err(format!(
            "auto calibration fail-closed: persistent {} checksum {:016x} != oracle {oracle:016x}",
            schedule.name(), evidence.measurement.timing.checksum
        ));
    }
    Ok(AutoCandidate {
        engine: match schedule {
            SchedulePolicy::PhysicalFirst => AutoEngine::PersistentPhysical,
            SchedulePolicy::Logical => AutoEngine::PersistentLogical,
        },
        tile_particles: Some(tile),
        effective_workers: worker_count,
        median_ns: evidence.measurement.timing.median_ns,
        startup_ns: evidence.pool_startup_ns,
        score_ns: candidate_score(
            evidence.measurement.timing.median_ns,
            evidence.pool_startup_ns,
            amortization_repeats,
        ),
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

    // Require an explicit 5% calibrated advantage before promotion. This makes
    // noisy near-ties resolve to the correctness-first canonical path.
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
    let calibration = calibration_config(full);
    let lut = Lut::build();
    let oracle = streaming_oracle_checksum(&calibration, &lut)?;
    let started = std::time::Instant::now();
    let mut candidates = Vec::new();

    candidates.push(calibrate_non_persistent(
        &calibration,
        AutoEngine::Canonical,
        None,
        oracle,
    )?);

    let tiles = auto_tiles(calibration.resident);
    for &tile in &tiles {
        candidates.push(calibrate_non_persistent(
            &calibration,
            AutoEngine::SpawnedSoa,
            Some(tile),
            oracle,
        )?);
    }

    let physical_workers = resolve_pool_workers(
        calibration.resident,
        calibration.requested_workers,
        topology,
        SchedulePolicy::PhysicalFirst,
    );
    let logical_workers = resolve_pool_workers(
        calibration.resident,
        calibration.requested_workers,
        topology,
        SchedulePolicy::Logical,
    );

    for &tile in &tiles {
        candidates.push(calibrate_persistent(
            &calibration,
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
                topology,
                SchedulePolicy::Logical,
                tile,
                oracle,
                full.repeats,
            )?);
        }
    }

    let selected = choose_candidate(&candidates)?;
    Ok((calibration, candidates, selected, started.elapsed().as_nanos()))
}

fn run_selected(full: &Config, topology: &TopologyInfo, selected: AutoCandidate) -> Result<AutoRunEvidence, String> {
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
                .engine
                .schedule()
                .ok_or_else(|| "persistent auto selection omitted schedule".to_string())?;
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

fn candidate_json(candidate: AutoCandidate) -> String {
    let schedule = candidate
        .engine
        .schedule()
        .map_or("null".to_string(), |schedule| format!("\"{}\"", schedule.name()));
    format!(
        "{{\"engine\":\"{}\",\"schedule\":{},\"tile_particles\":{},\"effective_workers\":{},\"median_ns\":{},\"startup_ns\":{},\"score_ns\":{},\"checksum\":\"{:016x}\"}}",
        candidate.engine.name(),
        schedule,
        optional_tile_json(candidate.tile_particles),
        candidate.effective_workers,
        candidate.median_ns,
        candidate.startup_ns,
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
    tuning_ns: u128,
    oracle_ns: u128,
    oracle_checksum: u64,
    evidence: AutoRunEvidence,
) -> String {
    let schedule = selected
        .engine
        .schedule()
        .map_or("null".to_string(), |schedule| format!("\"{}\"", schedule.name()));
    let candidate_blob = candidates
        .iter()
        .copied()
        .map(candidate_json)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\n  \"schema\": \"{AUTO_RECEIPT_SCHEMA}\",\n  \"runtime\": \"galaxy-cpu\",\n  \"execution_mode\": \"host-auto\",\n  \"selection_policy\": \"{AUTO_POLICY}\",\n  \"canonical_oracle\": \"{AUTO_ORACLE}\",\n  \"parity_fail_closed\": true,\n  \"promotion_margin_basis_points\": {AUTO_PROMOTION_MARGIN_BPS},\n  \"architecture\": \"{}\",\n  \"os\": \"{}\",\n  \"logical_population\": \"{}\",\n  \"resident_particles\": {},\n  \"frames\": {},\n  \"repeats\": {},\n  \"seed\": {},\n  \"requested_workers\": {},\n  \"available_parallelism\": {},\n  \"detected_physical_cores\": {},\n  \"topology_source\": \"{}\",\n  \"calibration_resident_particles\": {},\n  \"calibration_frames\": {},\n  \"calibration_repeats\": {},\n  \"calibration_candidate_count\": {},\n  \"calibration_candidates\": [{}],\n  \"tuning_ns\": {},\n  \"selected_engine\": \"{}\",\n  \"selected_schedule\": {},\n  \"selected_tile_particles\": {},\n  \"selected_effective_workers\": {},\n  \"selected_calibration_score_ns\": {},\n  \"selected_pool_startup_ns\": {},\n  \"full_oracle_ns\": {},\n  \"oracle_checksum\": \"{:016x}\",\n  \"selected_checksum\": \"{:016x}\",\n  \"checksum_match\": true,\n  \"best_ns\": {},\n  \"median_ns\": {},\n  \"peak_rss_kib\": {},\n  \"claim_boundary\": \"Host-aware selection is calibrated on a bounded deterministic slice and is not a universal hardware ranking. Promotion requires a 5% calibrated win over canonical. Every calibration candidate and the selected full-workload result must match the independent streaming canonical BAM-LUT oracle or execution fails closed.\"\n}}\n",
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
        calibration.resident,
        calibration.frames,
        calibration.repeats,
        candidates.len(),
        candidate_blob,
        tuning_ns,
        selected.engine.name(),
        schedule,
        optional_tile_json(selected.tile_particles),
        evidence.measurement.effective_workers,
        selected.score_ns,
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
    let topology = detect_topology();

    let (calibration, candidates, selected, tuning_ns) = calibrate_auto(&full, &topology)?;

    let oracle_lut = Lut::build();
    let oracle_started = std::time::Instant::now();
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
    println!("available_parallelism={}", topology.logical_cpus);
    match topology.physical_cores {
        Some(value) => println!("detected_physical_cores={value}"),
        None => println!("detected_physical_cores=unavailable"),
    }
    println!("topology_source={}", topology.source);
    println!("calibration_candidate_count={}", candidates.len());
    println!("tuning_ns={tuning_ns}");
    println!("selected_engine={}", selected.engine.name());
    match selected.engine.schedule() {
        Some(schedule) => println!("selected_schedule={}", schedule.name()),
        None => println!("selected_schedule=none"),
    }
    match selected.tile_particles {
        Some(tile) => println!("selected_tile_particles={tile}"),
        None => println!("selected_tile_particles=none"),
    }
    println!("selected_effective_workers={}", evidence.measurement.effective_workers);
    println!("selected_calibration_score_ns={}", selected.score_ns);
    println!("full_oracle_ns={oracle_ns}");
    println!("checksum={:016x}", evidence.measurement.timing.checksum);
    println!("best_ns={}", evidence.measurement.timing.best_ns);
    println!("median_ns={}", evidence.measurement.timing.median_ns);
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
                tuning_ns,
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

    #[test]
    fn promotion_margin_keeps_near_ties_canonical() {
        let canonical = AutoCandidate {
            engine: AutoEngine::Canonical,
            tile_particles: None,
            effective_workers: 8,
            median_ns: 1000,
            startup_ns: 0,
            score_ns: 1000,
            checksum: 1,
        };
        let near = AutoCandidate {
            engine: AutoEngine::SpawnedSoa,
            tile_particles: Some(1024),
            effective_workers: 8,
            median_ns: 960,
            startup_ns: 0,
            score_ns: 960,
            checksum: 1,
        };
        assert_eq!(choose_candidate(&[canonical, near]).unwrap().engine, AutoEngine::Canonical);
    }

    #[test]
    fn promotion_margin_allows_material_win() {
        let canonical = AutoCandidate {
            engine: AutoEngine::Canonical,
            tile_particles: None,
            effective_workers: 8,
            median_ns: 1000,
            startup_ns: 0,
            score_ns: 1000,
            checksum: 1,
        };
        let fast = AutoCandidate {
            engine: AutoEngine::PersistentPhysical,
            tile_particles: Some(1024),
            effective_workers: 8,
            median_ns: 800,
            startup_ns: 0,
            score_ns: 800,
            checksum: 1,
        };
        assert_eq!(choose_candidate(&[canonical, fast]).unwrap().engine, AutoEngine::PersistentPhysical);
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
