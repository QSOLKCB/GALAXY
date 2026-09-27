#![recursion_limit = "256"]
// SPDX-License-Identifier: Apache-2.0
use clap::Parser;
use galaxy_nbody::{
    flat::{build_flat_tree, flat_accelerations_from_tree},
    make_collision, make_disc, probe_error, Body, Config, Accel, DEFAULT_SOFTENING_KPC,
};
use galaxy_runtime::{
    config::Result,
    nbody_gpu::{GpuTreeEvidence, NbodyGpu, StageTiming, TreeBuildTiming},
};
use serde_json::json;
use std::{fs, path::PathBuf, time::Instant};

#[derive(Parser, Debug)]
#[command(about = "BH #2C GPU-built Barnes-Hut tree + evolving self-gravity")]
struct Args {
    #[arg(long, default_value = "disc")]
    preset: String,
    #[arg(long, default_value_t = 128)]
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
    #[arg(long)]
    adapter: Option<String>,
    #[arg(long)]
    allow_software: bool,
    #[arg(long)]
    receipt: Option<PathBuf>,
}

#[derive(Default)]
struct Timings {
    tree: TreeBuildTiming,
    force_transfer: f64,
    force_dispatch: f64,
    kick_drift_transfer: f64,
    kick_drift_dispatch: f64,
    final_kick_transfer: f64,
    final_kick_dispatch: f64,
    final_state_readback: f64,
    final_accel_readback: f64,
    cpu_trajectory_reference: f64,
    cpu_final_tree_reference: f64,
    cpu_final_force_reference: f64,
    direct_probe_reference: f64,
}

impl Timings {
    fn add_tree(&mut self, t: TreeBuildTiming) {
        self.tree.bounds_seconds += t.bounds_seconds;
        self.tree.morton_seconds += t.morton_seconds;
        self.tree.sort_seconds += t.sort_seconds;
        self.tree.positions_seconds += t.positions_seconds;
        self.tree.topology_seconds += t.topology_seconds;
        self.tree.aggregate_seconds += t.aggregate_seconds;
    }

    fn add_force(&mut self, t: StageTiming) {
        self.force_transfer += t.transfer_seconds;
        self.force_dispatch += t.dispatch_seconds;
    }

    fn add_kick_drift(&mut self, t: StageTiming) {
        self.kick_drift_transfer += t.transfer_seconds;
        self.kick_drift_dispatch += t.dispatch_seconds;
    }

    fn add_final_kick(&mut self, t: StageTiming) {
        self.final_kick_transfer += t.transfer_seconds;
        self.final_kick_dispatch += t.dispatch_seconds;
    }
}

fn make_bodies(args: &Args) -> Result<Vec<Body>> {
    match args.preset.as_str() {
        "disc" => Ok(make_disc(args.particles, args.seed, 5.0e10, 3.0)?),
        "collision" => Ok(make_collision(args.particles, args.seed)?),
        _ => Err("--preset must be disc or collision".into()),
    }
}

fn error_summary(approximate: &[Accel], reference: &[Accel]) -> Result<(f64, f64)> {
    if approximate.len() != reference.len() || approximate.is_empty() {
        return Err("force comparison requires equal non-empty arrays".into());
    }
    let mut sum_sq = 0.0;
    let mut max_relative = 0.0_f64;
    for (a, b) in approximate.iter().zip(reference) {
        let denominator = b.ax.hypot(b.ay).max(1e-30);
        let relative = (a.ax - b.ax).hypot(a.ay - b.ay) / denominator;
        sum_sq += relative * relative;
        max_relative = max_relative.max(relative);
    }
    Ok(((sum_sq / approximate.len() as f64).sqrt(), max_relative))
}

fn state_error(approximate: &[Body], reference: &[Body]) -> Result<(f64, f64, f64, f64)> {
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
    Ok(((p_err / p_ref.max(1e-30)).sqrt(), p_max, (v_err / v_ref.max(1e-30)).sqrt(), v_max))
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

fn gpu_tree_checksum(evidence: &GpuTreeEvidence) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for word in evidence.meta.data {
        checksum_word(&mut hash, word as u64);
    }
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

fn main() {
    if let Err(error) = run() {
        eprintln!("GALAXY BH #2C: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    if !(2..=4096).contains(&args.particles) {
        return Err("--particles must be in 2..=4096 for the BH #2C correctness-first builder".into());
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
    if !(1..=64).contains(&args.direct_probes) {
        return Err("--direct-probes must be in 1..=64".into());
    }

    let wall = Instant::now();
    let initial_bodies = make_bodies(&args)?;
    let config = Config {
        theta: args.theta,
        softening_kpc: args.softening_kpc,
        ..Config::default()
    };
    config.validate()?;

    let gpu = NbodyGpu::new(args.adapter.as_deref(), args.allow_software)?;
    let (evolving, state_create) = gpu.create_evolving_state(&initial_bodies)?;
    let tree = gpu.create_gpu_tree(&evolving)?;

    let mut timings = Timings::default();
    timings.add_tree(gpu.rebuild_gpu_tree(&evolving, &tree, config)?);
    timings.add_force(gpu.force_from_gpu_tree(&evolving, &tree, config)?);

    let initial_tree_evidence = gpu.read_gpu_tree_evidence(&tree)?;
    let initial_tree_checksum = gpu_tree_checksum(&initial_tree_evidence);

    for _ in 0..args.steps {
        timings.add_kick_drift(gpu.kick_drift(&evolving, args.dt_myr)?);
        timings.add_tree(gpu.rebuild_gpu_tree(&evolving, &tree, config)?);
        timings.add_force(gpu.force_from_gpu_tree(&evolving, &tree, config)?);
        timings.add_final_kick(gpu.final_kick(&evolving, args.dt_myr)?);
    }

    let (final_bodies, state_readback) = gpu.read_evolving_state(&evolving)?;
    timings.final_state_readback = state_readback.readback_seconds;
    let (final_acceleration, accel_readback) = gpu.read_evolving_accelerations(&evolving)?;
    timings.final_accel_readback = accel_readback.readback_seconds;

    let final_tree_evidence = gpu.read_gpu_tree_evidence(&tree)?;
    let final_tree_checksum = gpu_tree_checksum(&final_tree_evidence);
    if final_tree_evidence.meta.data[3] != 0 {
        return Err("GPU tree overflow flag is set".into());
    }

    let repeat_timing = gpu.rebuild_gpu_tree(&evolving, &tree, config)?;
    let repeat_tree_evidence = gpu.read_gpu_tree_evidence(&tree)?;
    let repeat_tree_checksum = gpu_tree_checksum(&repeat_tree_evidence);
    let deterministic_tree_rebuild = final_tree_checksum == repeat_tree_checksum;

    let direct_count = args.direct_probes.min(args.particles);
    let direct_started = Instant::now();
    let direct = probe_error(&final_bodies, &final_acceleration, config, direct_count)?;
    timings.direct_probe_reference = direct_started.elapsed().as_secs_f64();
    if direct.rms_relative >= 0.04 || direct.max_relative >= 0.30 {
        return Err(format!(
            "BH #2C final direct-force error too large: rms={} max={}",
            direct.rms_relative, direct.max_relative
        ).into());
    }

    let cpu_tree_started = Instant::now();
    let cpu_tree = build_flat_tree(&final_bodies, config)?;
    timings.cpu_final_tree_reference = cpu_tree_started.elapsed().as_secs_f64();
    let cpu_force_started = Instant::now();
    let (cpu_force, _) = flat_accelerations_from_tree(&final_bodies, &cpu_tree, config)?;
    timings.cpu_final_force_reference = cpu_force_started.elapsed().as_secs_f64();
    let (gpu_cpu_force_rms, gpu_cpu_force_max) = error_summary(&final_acceleration, &cpu_force)?;
    if gpu_cpu_force_rms >= 0.04 || gpu_cpu_force_max >= 0.30 {
        return Err(format!(
            "BH #2C force diverged from BH #2A flat oracle: rms={gpu_cpu_force_rms} max={gpu_cpu_force_max}"
        ).into());
    }

    let cpu_trajectory_started = Instant::now();
    let cpu_trajectory = cpu_flat_evolve(initial_bodies.clone(), args.steps, args.dt_myr, config)?;
    timings.cpu_trajectory_reference = cpu_trajectory_started.elapsed().as_secs_f64();
    let (pos_rms, pos_max, vel_rms, vel_max) = state_error(&final_bodies, &cpu_trajectory)?;
    if pos_rms >= 0.03 || pos_max >= 0.30 || vel_rms >= 0.03 || vel_max >= 0.30 {
        return Err(format!(
            "BH #2C trajectory diverged from BH #2A reference: pos_rms={pos_rms} pos_max={pos_max} vel_rms={vel_rms} vel_max={vel_max}"
        ).into());
    }
    if !deterministic_tree_rebuild {
        return Err("rebuilding the unchanged final GPU state changed the GPU tree checksum".into());
    }

    let receipt = json!({
        "schema": "galaxy.barnes-hut-gpu-tree-evolution-receipt.v1",
        "status": "complete",
        "phase": "BH-2C",
        "builder": "gpu-f32-serialized-control-v1",
        "preset": args.preset,
        "particles": args.particles,
        "steps": args.steps,
        "dt_myr": args.dt_myr,
        "simulated_time_myr": args.dt_myr * args.steps as f64,
        "seed": args.seed,
        "theta": args.theta,
        "softening_kpc": args.softening_kpc,
        "gpu": gpu.info(),
        "force_solves": args.steps + 1,
        "gpu_tree_builds": args.steps + 2,
        "evolution_tree_builds": args.steps + 1,
        "host_tree_rebuilds": 0,
        "host_particle_readbacks_during_steps": 0,
        "persistent_gpu_state_bytes": gpu.evolving_buffer_bytes(&evolving),
        "gpu_tree_buffer_bytes": gpu.gpu_tree_buffer_bytes(&tree),
        "tree": {
            "initial_checksum_fnv_mix64": format!("{initial_tree_checksum:016x}"),
            "final_checksum_fnv_mix64": format!("{final_tree_checksum:016x}"),
            "repeat_checksum_fnv_mix64": format!("{repeat_tree_checksum:016x}"),
            "repeat_rebuild_matches": deterministic_tree_rebuild,
            "cell_count": final_tree_evidence.meta.data[0],
            "leaf_count": final_tree_evidence.meta.data[1],
            "max_depth": final_tree_evidence.meta.data[2],
            "overflow": final_tree_evidence.meta.data[3],
            "root_bounds_f32": final_tree_evidence.meta.bounds
        },
        "final_force": {
            "gpu_vs_bh2a_flat_rms_relative": gpu_cpu_force_rms,
            "gpu_vs_bh2a_flat_max_relative": gpu_cpu_force_max,
            "direct_probe_count": direct_count,
            "direct_probe_rms_relative": direct.rms_relative,
            "direct_probe_max_relative": direct.max_relative
        },
        "trajectory_vs_bh2a_flat_f64": {
            "position_rms_relative_l2": pos_rms,
            "position_max_relative": pos_max,
            "velocity_rms_relative_l2": vel_rms,
            "velocity_max_relative": vel_max
        },
        "timings_seconds": {
            "gpu_state_create_transfer": state_create.transfer_seconds,
            "gpu_tree_bounds_total": timings.tree.bounds_seconds,
            "gpu_tree_morton_total": timings.tree.morton_seconds,
            "gpu_tree_sort_total": timings.tree.sort_seconds,
            "gpu_tree_positions_total": timings.tree.positions_seconds,
            "gpu_tree_topology_total": timings.tree.topology_seconds,
            "gpu_tree_aggregate_total": timings.tree.aggregate_seconds,
            "gpu_tree_build_total": timings.tree.total_seconds(),
            "gpu_repeat_tree_build": repeat_timing.total_seconds(),
            "gpu_force_transfer_total": timings.force_transfer,
            "gpu_force_dispatch_total": timings.force_dispatch,
            "gpu_kick_drift_uniform_transfer_total": timings.kick_drift_transfer,
            "gpu_kick_drift_dispatch_total": timings.kick_drift_dispatch,
            "gpu_final_kick_uniform_transfer_total": timings.final_kick_transfer,
            "gpu_final_kick_dispatch_total": timings.final_kick_dispatch,
            "final_state_readback": timings.final_state_readback,
            "final_acceleration_readback": timings.final_accel_readback,
            "cpu_final_tree_reference": timings.cpu_final_tree_reference,
            "cpu_final_force_reference": timings.cpu_final_force_reference,
            "cpu_trajectory_reference": timings.cpu_trajectory_reference,
            "direct_probe_reference": timings.direct_probe_reference,
            "total_wall": wall.elapsed().as_secs_f64()
        },
        "scope": "BH #2C correctness-first GPU tree construction: bounds, Morton generation, deterministic bitonic ordering, flat topology and aggregates execute on GPU; no host tree rebuild occurs during evolution; control-heavy build stages are serialized GPU kernels and are not a performance claim"
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
