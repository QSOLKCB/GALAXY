#![recursion_limit = "256"]
// SPDX-License-Identifier: Apache-2.0
use clap::Parser;
use galaxy_nbody::{
    flat::{build_flat_tree, flat_accelerations_from_tree},
    make_collision, make_disc, probe_error, Accel, Body, Config, DEFAULT_SOFTENING_KPC,
};
use galaxy_runtime::{
    config::Result,
    nbody_gpu::{pack_flat_tree, NbodyGpu, StageTiming, TreeBuildTiming},
    nbody_parallel::{
        ParallelGpuTreeEvidence, ParallelTreeBuildTiming, ParallelTreeRuntime,
        MAX_PARALLEL_TREE_BODIES,
    },
};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, time::Instant};

#[derive(Parser, Debug)]
#[command(about = "BH #2D parallel GPU Barnes-Hut tree construction and evolution")]
struct Args {
    #[arg(long, default_value = "disc")]
    preset: String,
    #[arg(long, default_value_t = 512)]
    particles: usize,
    #[arg(long, default_value_t = 3)]
    steps: usize,
    #[arg(long, default_value_t = 0.01)]
    dt_myr: f64,
    #[arg(long, default_value_t = 303)]
    seed: u64,
    #[arg(long, default_value_t = 0.5)]
    theta: f64,
    #[arg(long, default_value_t = DEFAULT_SOFTENING_KPC)]
    softening_kpc: f64,
    #[arg(long, default_value_t = 12)]
    direct_probes: usize,
    #[arg(long, default_value_t = 4096)]
    oracle_limit: usize,
    #[arg(long, default_value_t = 1)]
    benchmark_warmup: usize,
    #[arg(long, default_value_t = 5)]
    benchmark_repeats: usize,
    #[arg(long)]
    adapter: Option<String>,
    #[arg(long)]
    allow_software: bool,
    #[arg(long)]
    require_hardware: bool,
    #[arg(long)]
    receipt: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
struct Error4 {
    position_rms: f64,
    position_max: f64,
    velocity_rms: f64,
    velocity_max: f64,
}

fn state_error(approximate: &[Body], reference: &[Body]) -> Result<Error4> {
    if approximate.len() != reference.len() || approximate.is_empty() {
        return Err("state comparison requires equal non-empty arrays".into());
    }
    let mut p_err = 0.0;
    let mut p_ref = 0.0;
    let mut v_err = 0.0;
    let mut v_ref = 0.0;
    let mut p_max = 0.0_f64;
    let mut v_max = 0.0_f64;
    for (a, b) in approximate.iter().zip(reference) {
        let pd = (a.x - b.x).hypot(a.y - b.y);
        let vd = (a.vx - b.vx).hypot(a.vy - b.vy);
        let pr = b.x.hypot(b.y);
        let vr = b.vx.hypot(b.vy);
        p_err += pd * pd;
        p_ref += pr * pr;
        v_err += vd * vd;
        v_ref += vr * vr;
        p_max = p_max.max(pd / pr.max(1e-6));
        v_max = v_max.max(vd / vr.max(1e-9));
    }
    Ok(Error4 {
        position_rms: (p_err / p_ref.max(1e-30)).sqrt(),
        position_max: p_max,
        velocity_rms: (v_err / v_ref.max(1e-30)).sqrt(),
        velocity_max: v_max,
    })
}

fn error4_json(error: Error4) -> Value {
    json!({
        "position_rms_relative_l2": error.position_rms,
        "position_max_relative": error.position_max,
        "velocity_rms_relative_l2": error.velocity_rms,
        "velocity_max_relative": error.velocity_max
    })
}

fn force_error(approximate: &[Accel], reference: &[Accel]) -> Result<(f64, f64)> {
    if approximate.len() != reference.len() || approximate.is_empty() {
        return Err("force comparison requires equal non-empty arrays".into());
    }
    let mut sum_sq = 0.0;
    let mut max_relative = 0.0_f64;
    for (a, b) in approximate.iter().zip(reference) {
        let denom = b.ax.hypot(b.ay).max(1e-30);
        let relative = (a.ax - b.ax).hypot(a.ay - b.ay) / denom;
        sum_sq += relative * relative;
        max_relative = max_relative.max(relative);
    }
    Ok(((sum_sq / approximate.len() as f64).sqrt(), max_relative))
}

fn make_bodies(args: &Args) -> Result<Vec<Body>> {
    match args.preset.as_str() {
        "disc" => Ok(make_disc(args.particles, args.seed, 5.0e10, 3.0)?),
        "collision" => Ok(make_collision(args.particles, args.seed)?),
        _ => Err("--preset must be disc or collision".into()),
    }
}

fn cpu_flat_evolve(
    mut bodies: Vec<Body>,
    steps: usize,
    dt_myr: f64,
    config: Config,
) -> Result<Vec<Body>> {
    let mut tree = build_flat_tree(&bodies, config)?;
    let (mut acceleration, _) = flat_accelerations_from_tree(&bodies, &tree, config)?;
    for _ in 0..steps {
        for (body, acc) in bodies.iter_mut().zip(&acceleration) {
            body.vx += 0.5 * dt_myr * acc.ax;
            body.vy += 0.5 * dt_myr * acc.ay;
            body.x += dt_myr * body.vx;
            body.y += dt_myr * body.vy;
        }
        tree = build_flat_tree(&bodies, config)?;
        let (next, _) = flat_accelerations_from_tree(&bodies, &tree, config)?;
        for (body, acc) in bodies.iter_mut().zip(&next) {
            body.vx += 0.5 * dt_myr * acc.ax;
            body.vy += 0.5 * dt_myr * acc.ay;
        }
        acceleration = next;
    }
    Ok(bodies)
}

fn checksum_word(hash: &mut u64, word: u64) {
    *hash ^= word;
    *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    *hash ^= word.rotate_left(23);
}

fn parallel_tree_checksum(evidence: &ParallelGpuTreeEvidence) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for entry in &evidence.entries {
        checksum_word(&mut hash, entry.data[0] as u64);
        checksum_word(&mut hash, entry.data[1] as u64);
    }
    for cell in &evidence.cells {
        checksum_word(&mut hash, cell.range_depth[0] as u64);
        checksum_word(&mut hash, cell.range_depth[1] as u64);
        checksum_word(&mut hash, cell.range_depth[2] as u64);
        for child in cell.children {
            checksum_word(&mut hash, child as u64);
        }
    }
    hash
}

fn validate_parallel_order(evidence: &ParallelGpuTreeEvidence) -> Result<()> {
    for pair in evidence.entries.windows(2) {
        let a = pair[0].data;
        let b = pair[1].data;
        if a[0] > b[0] || (a[0] == b[0] && a[1] >= b[1]) {
            return Err(format!(
                "BH #2D radix order is not strict by (Morton code, body index): {:?} then {:?}",
                &a[..2],
                &b[..2]
            )
            .into());
        }
    }
    Ok(())
}

fn add_parallel(total: &mut ParallelTreeBuildTiming, value: ParallelTreeBuildTiming) {
    total.bounds_seconds += value.bounds_seconds;
    total.morton_seconds += value.morton_seconds;
    total.radix_histogram_seconds += value.radix_histogram_seconds;
    total.radix_prefix_seconds += value.radix_prefix_seconds;
    total.radix_scatter_seconds += value.radix_scatter_seconds;
    total.positions_seconds += value.positions_seconds;
    total.topology_seconds += value.topology_seconds;
    total.aggregate_seconds += value.aggregate_seconds;
}

fn add_stage(total: &mut StageTiming, value: StageTiming) {
    total.transfer_seconds += value.transfer_seconds;
    total.dispatch_seconds += value.dispatch_seconds;
    total.readback_seconds += value.readback_seconds;
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.total_cmp(b));
    values[values.len() / 2]
}

fn run_serial_oracle(
    gpu: &NbodyGpu,
    initial: &[Body],
    steps: usize,
    dt_myr: f64,
    config: Config,
) -> Result<(Vec<Body>, Vec<Accel>)> {
    let (evolving, _) = gpu.create_evolving_state(initial)?;
    let tree = gpu.create_gpu_tree(&evolving)?;
    gpu.rebuild_gpu_tree(&evolving, &tree, config)?;
    gpu.force_from_gpu_tree(&evolving, &tree, config)?;
    for _ in 0..steps {
        gpu.kick_drift(&evolving, dt_myr)?;
        gpu.rebuild_gpu_tree(&evolving, &tree, config)?;
        gpu.force_from_gpu_tree(&evolving, &tree, config)?;
        gpu.final_kick(&evolving, dt_myr)?;
    }
    let (state, _) = gpu.read_evolving_state(&evolving)?;
    let (acceleration, _) = gpu.read_evolving_accelerations(&evolving)?;
    Ok((state, acceleration))
}

fn run_host_oracle(
    gpu: &NbodyGpu,
    initial: &[Body],
    steps: usize,
    dt_myr: f64,
    config: Config,
) -> Result<(Vec<Body>, Vec<Accel>)> {
    let (evolving, _) = gpu.create_evolving_state(initial)?;
    let initial_tree = build_flat_tree(initial, config)?;
    let initial_packed = pack_flat_tree(initial, &initial_tree, config)?;
    gpu.force_into(&evolving, &initial_packed)?;

    for _ in 0..steps {
        gpu.kick_drift(&evolving, dt_myr)?;
        let (drifted, _) = gpu.read_evolving_state(&evolving)?;
        let tree = build_flat_tree(&drifted, config)?;
        let packed = pack_flat_tree(&drifted, &tree, config)?;
        gpu.force_into(&evolving, &packed)?;
        gpu.final_kick(&evolving, dt_myr)?;
    }

    let (state, _) = gpu.read_evolving_state(&evolving)?;
    let (acceleration, _) = gpu.read_evolving_accelerations(&evolving)?;
    Ok((state, acceleration))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("GALAXY BH #2D: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    if !(2..=MAX_PARALLEL_TREE_BODIES as usize).contains(&args.particles) {
        return Err(format!(
            "--particles must be in 2..={MAX_PARALLEL_TREE_BODIES} for BH #2D"
        )
        .into());
    }
    if args.preset == "collision" && args.particles < 4 {
        return Err("collision preset requires at least 4 particles".into());
    }
    if !(1..=256).contains(&args.steps) {
        return Err("--steps must be in 1..=256".into());
    }
    if !args.dt_myr.is_finite() || !(1e-6..=1.0).contains(&args.dt_myr) {
        return Err("--dt-myr must be finite and in [1e-6, 1]".into());
    }
    if !args.theta.is_finite() || !(0.0..=2.0).contains(&args.theta) {
        return Err("--theta must be finite and in [0, 2]".into());
    }
    if !args.softening_kpc.is_finite() || !(0.0..=100.0).contains(&args.softening_kpc) {
        return Err("--softening-kpc must be finite and in [0, 100]".into());
    }
    if !(1..=64).contains(&args.direct_probes) {
        return Err("--direct-probes must be in 1..=64".into());
    }
    if !(2..=4096).contains(&args.oracle_limit) {
        return Err("--oracle-limit must be in 2..=4096".into());
    }
    if args.benchmark_repeats == 0 || args.benchmark_repeats > 31 || args.benchmark_warmup > 10 {
        return Err("benchmark warmup/repeats must be warmup<=10 and repeats in 1..=31".into());
    }

    let initial = make_bodies(&args)?;
    let config = Config {
        theta: args.theta,
        softening_kpc: args.softening_kpc,
        ..Config::default()
    };
    config.validate()?;

    let gpu = NbodyGpu::new(args.adapter.as_deref(), args.allow_software)?;
    let gpu_info = gpu.info();
    let software = gpu_info["software"].as_bool().unwrap_or(false);
    if args.require_hardware && software {
        return Err("--require-hardware rejected the selected software GPU adapter".into());
    }

    let runtime = ParallelTreeRuntime::new(&gpu)?;
    let (evolving, state_create) = gpu.create_evolving_state(&initial)?;
    let tree = runtime.create_tree(&gpu, &evolving)?;

    let wall_started = Instant::now();
    let mut build_timing = ParallelTreeBuildTiming::default();
    let mut force_timing = StageTiming::default();
    let mut kick_drift_timing = StageTiming::default();
    let mut final_kick_timing = StageTiming::default();

    add_parallel(&mut build_timing, runtime.rebuild(&gpu, &evolving, &tree, config)?);
    add_stage(&mut force_timing, runtime.force(&gpu, &evolving, &tree, config)?);

    let initial_evidence = runtime.read_evidence(&gpu, &tree)?;
    validate_parallel_order(&initial_evidence)?;
    let initial_tree_checksum = parallel_tree_checksum(&initial_evidence);

    for _ in 0..args.steps {
        add_stage(&mut kick_drift_timing, gpu.kick_drift(&evolving, args.dt_myr)?);
        add_parallel(&mut build_timing, runtime.rebuild(&gpu, &evolving, &tree, config)?);
        add_stage(&mut force_timing, runtime.force(&gpu, &evolving, &tree, config)?);
        add_stage(&mut final_kick_timing, gpu.final_kick(&evolving, args.dt_myr)?);
    }

    let (final_bodies, final_state_readback) = gpu.read_evolving_state(&evolving)?;
    let (final_acceleration, final_accel_readback) =
        gpu.read_evolving_accelerations(&evolving)?;

    let final_evidence = runtime.read_evidence(&gpu, &tree)?;
    validate_parallel_order(&final_evidence)?;
    let final_tree_checksum = parallel_tree_checksum(&final_evidence);

    runtime.rebuild(&gpu, &evolving, &tree, config)?;
    let repeat_evidence = runtime.read_evidence(&gpu, &tree)?;
    validate_parallel_order(&repeat_evidence)?;
    let repeat_tree_checksum = parallel_tree_checksum(&repeat_evidence);
    if repeat_tree_checksum != final_tree_checksum {
        return Err("BH #2D unchanged-state repeat tree checksum changed".into());
    }

    let direct_count = args.direct_probes.min(args.particles);
    let direct_started = Instant::now();
    let direct = probe_error(&final_bodies, &final_acceleration, config, direct_count)?;
    let direct_seconds = direct_started.elapsed().as_secs_f64();
    if direct.rms_relative >= 0.04 || direct.max_relative >= 0.30 {
        return Err(format!(
            "BH #2D direct-force gate failed: rms={} max={}",
            direct.rms_relative, direct.max_relative
        )
        .into());
    }

    let cpu_tree_started = Instant::now();
    let cpu_tree = build_flat_tree(&final_bodies, config)?;
    let cpu_tree_seconds = cpu_tree_started.elapsed().as_secs_f64();
    let cpu_force_started = Instant::now();
    let (cpu_force, _) = flat_accelerations_from_tree(&final_bodies, &cpu_tree, config)?;
    let cpu_force_seconds = cpu_force_started.elapsed().as_secs_f64();
    let (parallel_cpu_rms, parallel_cpu_max) = force_error(&final_acceleration, &cpu_force)?;
    if parallel_cpu_rms >= 0.04 || parallel_cpu_max >= 0.30 {
        return Err(format!(
            "BH #2D force diverged from BH #2A flat oracle: rms={parallel_cpu_rms} max={parallel_cpu_max}"
        )
        .into());
    }

    let cpu_trajectory_started = Instant::now();
    let cpu_trajectory = cpu_flat_evolve(initial.clone(), args.steps, args.dt_myr, config)?;
    let cpu_trajectory_seconds = cpu_trajectory_started.elapsed().as_secs_f64();
    let cpu_state_error = state_error(&final_bodies, &cpu_trajectory)?;
    if cpu_state_error.position_rms >= 0.03
        || cpu_state_error.position_max >= 0.30
        || cpu_state_error.velocity_rms >= 0.03
        || cpu_state_error.velocity_max >= 0.30
    {
        return Err(format!("BH #2D trajectory diverged from BH #2A: {cpu_state_error:?}").into());
    }

    let oracle_evidence = if args.particles <= args.oracle_limit {
        let (serial_state, serial_accel) =
            run_serial_oracle(&gpu, &initial, args.steps, args.dt_myr, config)?;
        let (host_state, host_accel) =
            run_host_oracle(&gpu, &initial, args.steps, args.dt_myr, config)?;

        let serial_state_error = state_error(&final_bodies, &serial_state)?;
        let host_state_error = state_error(&final_bodies, &host_state)?;
        let (serial_force_rms, serial_force_max) =
            force_error(&final_acceleration, &serial_accel)?;
        let (host_force_rms, host_force_max) =
            force_error(&final_acceleration, &host_accel)?;

        for (name, error) in [
            ("BH #2C state", serial_state_error),
            ("BH #2B2 state", host_state_error),
        ] {
            if error.position_rms >= 0.03
                || error.position_max >= 0.30
                || error.velocity_rms >= 0.03
                || error.velocity_max >= 0.30
            {
                return Err(format!("{name} parity failed: {error:?}").into());
            }
        }
        if serial_force_rms >= 0.04
            || serial_force_max >= 0.30
            || host_force_rms >= 0.04
            || host_force_max >= 0.30
        {
            return Err("BH #2D force parity failed against BH #2C/BH #2B2".into());
        }

        json!({
            "status": "executed",
            "bh2c_serial_gpu": {
                "state_error": error4_json(serial_state_error),
                "force_rms_relative": serial_force_rms,
                "force_max_relative": serial_force_max
            },
            "bh2b2_host_tree_gpu": {
                "state_error": error4_json(host_state_error),
                "force_rms_relative": host_force_rms,
                "force_max_relative": host_force_max
            }
        })
    } else {
        json!({
            "status": "skipped-particle-limit",
            "limit": args.oracle_limit,
            "reason": "BH #2C serialized GPU oracle is intentionally capped at 4096 bodies"
        })
    };

    // Same-state build benchmark. Software adapters are validation only;
    // hardware adapters may be used as measured performance evidence.
    for _ in 0..args.benchmark_warmup {
        runtime.rebuild(&gpu, &evolving, &tree, config)?;
    }
    let mut parallel_samples = Vec::with_capacity(args.benchmark_repeats);
    for _ in 0..args.benchmark_repeats {
        parallel_samples.push(runtime.rebuild(&gpu, &evolving, &tree, config)?.total_seconds());
    }
    let parallel_median = median(parallel_samples.clone());

    let serial_benchmark = if args.particles <= 4096 {
        let serial_tree = gpu.create_gpu_tree(&evolving)?;
        for _ in 0..args.benchmark_warmup {
            gpu.rebuild_gpu_tree(&evolving, &serial_tree, config)?;
        }
        let mut serial_samples = Vec::with_capacity(args.benchmark_repeats);
        for _ in 0..args.benchmark_repeats {
            serial_samples.push(
                gpu.rebuild_gpu_tree(&evolving, &serial_tree, config)?
                    .total_seconds(),
            );
        }
        let serial_median = median(serial_samples.clone());
        json!({
            "status": "executed",
            "samples_seconds": serial_samples,
            "median_seconds": serial_median,
            "parallel_vs_serial_speedup": serial_median / parallel_median
        })
    } else {
        json!({
            "status": "skipped-bh2c-cap",
            "limit": 4096
        })
    };

    let receipt = json!({
        "schema": "galaxy.barnes-hut-parallel-gpu-tree-receipt.v1",
        "status": "complete",
        "phase": "BH-2D",
        "builder": "gpu-parallel-sparse-radix-v1",
        "measurement_class": if software { "software-validation" } else { "hardware" },
        "hardware_performance_claim_allowed": !software,
        "preset": args.preset,
        "particles": args.particles,
        "steps": args.steps,
        "dt_myr": args.dt_myr,
        "simulated_time_myr": args.dt_myr * args.steps as f64,
        "seed": args.seed,
        "theta": args.theta,
        "softening_kpc": args.softening_kpc,
        "gpu": gpu_info,
        "host_tree_rebuilds": 0,
        "host_particle_readbacks_during_steps": 0,
        "force_solves": args.steps + 1,
        "evolution_tree_builds": args.steps + 1,
        "parallel_tree_buffer_bytes": runtime.buffer_bytes(&tree),
        "tree": {
            "initial_checksum_fnv_mix64": format!("{initial_tree_checksum:016x}"),
            "final_checksum_fnv_mix64": format!("{final_tree_checksum:016x}"),
            "repeat_checksum_fnv_mix64": format!("{repeat_tree_checksum:016x}"),
            "repeat_rebuild_matches": repeat_tree_checksum == final_tree_checksum,
            "active_cell_count": final_evidence.active_cell_count,
            "leaf_count": final_evidence.leaf_count,
            "max_depth": final_evidence.max_depth,
            "root_bounds_f32": final_evidence.meta.bounds,
            "ordering": "stable-lsd-radix-4bit-morton-code; resident body index retained for equal Morton keys",
            "layout": "sparse-level-order-slot=depth*N+group_start"
        },
        "final_force": {
            "gpu_vs_bh2a_flat_rms_relative": parallel_cpu_rms,
            "gpu_vs_bh2a_flat_max_relative": parallel_cpu_max,
            "direct_probe_count": direct_count,
            "direct_probe_rms_relative": direct.rms_relative,
            "direct_probe_max_relative": direct.max_relative
        },
        "trajectory_vs_bh2a_flat_f64": error4_json(cpu_state_error),
        "gpu_oracles": oracle_evidence,
        "tree_build_benchmark": {
            "warmup": args.benchmark_warmup,
            "repeats": args.benchmark_repeats,
            "parallel_samples_seconds": parallel_samples,
            "parallel_median_seconds": parallel_median,
            "bh2c_serial": serial_benchmark
        },
        "timings_seconds": {
            "gpu_state_create_transfer": state_create.transfer_seconds,
            "parallel_bounds_total": build_timing.bounds_seconds,
            "parallel_morton_total": build_timing.morton_seconds,
            "parallel_radix_histogram_total": build_timing.radix_histogram_seconds,
            "parallel_radix_prefix_total": build_timing.radix_prefix_seconds,
            "parallel_radix_scatter_total": build_timing.radix_scatter_seconds,
            "parallel_radix_total": build_timing.radix_seconds(),
            "parallel_positions_total": build_timing.positions_seconds,
            "parallel_topology_total": build_timing.topology_seconds,
            "parallel_aggregate_total": build_timing.aggregate_seconds,
            "parallel_tree_build_total": build_timing.total_seconds(),
            "gpu_force_transfer_total": force_timing.transfer_seconds,
            "gpu_force_dispatch_total": force_timing.dispatch_seconds,
            "gpu_kick_drift_transfer_total": kick_drift_timing.transfer_seconds,
            "gpu_kick_drift_dispatch_total": kick_drift_timing.dispatch_seconds,
            "gpu_final_kick_transfer_total": final_kick_timing.transfer_seconds,
            "gpu_final_kick_dispatch_total": final_kick_timing.dispatch_seconds,
            "final_state_readback": final_state_readback.readback_seconds,
            "final_acceleration_readback": final_accel_readback.readback_seconds,
            "cpu_final_tree_reference": cpu_tree_seconds,
            "cpu_final_force_reference": cpu_force_seconds,
            "cpu_trajectory_reference": cpu_trajectory_seconds,
            "direct_probe_reference": direct_seconds,
            "total_wall": wall_started.elapsed().as_secs_f64()
        },
        "scope": "BH #2D parallel GPU tree construction with workgroup bounds reduction, stable block-parallel radix ordering, parallel sparse range/topology construction and reverse-depth parallel aggregates. BH #2C and BH #2B2 remain executable oracles. Software-Vulkan timings are validation only; hardware performance claims require a non-software adapter."
    });

    if let Some(path) = args.receipt {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serde_json::to_vec_pretty(&receipt)?)?;
    }
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}
