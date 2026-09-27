// SPDX-License-Identifier: Apache-2.0
use clap::Parser;
use galaxy_nbody::{
    flat::{build_flat_tree, flat_accelerations_from_tree, flat_topology_checksum},
    make_collision, make_disc, probe_error, state_checksum, Accel, Body, Config,
    DEFAULT_SOFTENING_KPC,
};
use galaxy_runtime::{
    config::Result,
    nbody_gpu::{pack_flat_tree, NbodyGpu, StageTiming},
};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, time::Instant};

#[derive(Parser, Debug)]
#[command(about = "BH #2B2 evolving Barnes-Hut self-gravity on the native GPU runtime")]
struct Args {
    #[arg(long, default_value = "disc")]
    preset: String,
    #[arg(long, default_value_t = 256)]
    particles: usize,
    #[arg(long, default_value_t = 4)]
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
    cpu_reference_limit: usize,
    #[arg(long)]
    adapter: Option<String>,
    #[arg(long)]
    allow_software: bool,
    #[arg(long)]
    receipt: Option<PathBuf>,
}

#[derive(Default)]
struct Timings {
    cpu_initial_tree_build: f64,
    cpu_tree_rebuild: f64,
    cpu_pack_f32: f64,
    gpu_state_create_transfer: f64,
    gpu_force_transfer: f64,
    gpu_force_dispatch: f64,
    gpu_kick_drift_transfer: f64,
    gpu_kick_drift_dispatch: f64,
    gpu_drift_readback: f64,
    gpu_final_kick_transfer: f64,
    gpu_final_kick_dispatch: f64,
    gpu_final_state_readback: f64,
    gpu_final_acceleration_readback: f64,
    direct_probe_reference: f64,
    cpu_full_reference: f64,
}

impl Timings {
    fn add_force(&mut self, timing: StageTiming) {
        self.gpu_force_transfer += timing.transfer_seconds;
        self.gpu_force_dispatch += timing.dispatch_seconds;
    }

    fn add_kick_drift(&mut self, timing: StageTiming) {
        self.gpu_kick_drift_transfer += timing.transfer_seconds;
        self.gpu_kick_drift_dispatch += timing.dispatch_seconds;
    }

    fn add_final_kick(&mut self, timing: StageTiming) {
        self.gpu_final_kick_transfer += timing.transfer_seconds;
        self.gpu_final_kick_dispatch += timing.dispatch_seconds;
    }
}

#[derive(Clone, Copy)]
struct Invariants {
    mass: f64,
    com_x: f64,
    com_y: f64,
    px: f64,
    py: f64,
    lz: f64,
    mass_speed_scale: f64,
}

fn invariants(bodies: &[Body]) -> Invariants {
    let mut mass = 0.0;
    let mut wx = 0.0;
    let mut wy = 0.0;
    let mut px = 0.0;
    let mut py = 0.0;
    let mut lz = 0.0;
    let mut mass_speed_scale = 0.0;
    for body in bodies {
        mass += body.mass;
        wx += body.mass * body.x;
        wy += body.mass * body.y;
        px += body.mass * body.vx;
        py += body.mass * body.vy;
        lz += body.mass * (body.x * body.vy - body.y * body.vx);
        mass_speed_scale += body.mass * body.vx.hypot(body.vy);
    }
    Invariants {
        mass,
        com_x: wx / mass,
        com_y: wy / mass,
        px,
        py,
        lz,
        mass_speed_scale,
    }
}

fn invariant_json(initial: Invariants, final_state: Invariants) -> Value {
    let momentum_delta = (final_state.px - initial.px).hypot(final_state.py - initial.py);
    json!({
        "initial": {
            "mass_msun": initial.mass,
            "center_of_mass_kpc": [initial.com_x, initial.com_y],
            "linear_momentum_msun_kpc_per_myr": [initial.px, initial.py],
            "angular_momentum_z_msun_kpc2_per_myr": initial.lz
        },
        "final": {
            "mass_msun": final_state.mass,
            "center_of_mass_kpc": [final_state.com_x, final_state.com_y],
            "linear_momentum_msun_kpc_per_myr": [final_state.px, final_state.py],
            "angular_momentum_z_msun_kpc2_per_myr": final_state.lz
        },
        "drift": {
            "mass_relative": (final_state.mass - initial.mass).abs() / initial.mass.abs().max(1e-30),
            "center_of_mass_kpc": (final_state.com_x - initial.com_x)
                .hypot(final_state.com_y - initial.com_y),
            "linear_momentum_fraction_of_initial_mass_speed": momentum_delta
                / initial.mass_speed_scale.max(1e-30),
            "angular_momentum_relative": (final_state.lz - initial.lz).abs()
                / initial.lz.abs().max(1e-30)
        }
    })
}

fn state_error(approximate: &[Body], reference: &[Body]) -> Result<Value> {
    if approximate.len() != reference.len() || approximate.is_empty() {
        return Err("state comparison requires equal non-empty body arrays".into());
    }
    let mut position_error_sq = 0.0;
    let mut position_reference_sq = 0.0;
    let mut velocity_error_sq = 0.0;
    let mut velocity_reference_sq = 0.0;
    let mut position_max_relative = 0.0_f64;
    let mut velocity_max_relative = 0.0_f64;

    for (a, b) in approximate.iter().zip(reference) {
        let pd = (a.x - b.x).hypot(a.y - b.y);
        let vd = (a.vx - b.vx).hypot(a.vy - b.vy);
        let pr = b.x.hypot(b.y);
        let vr = b.vx.hypot(b.vy);
        position_error_sq += pd * pd;
        position_reference_sq += pr * pr;
        velocity_error_sq += vd * vd;
        velocity_reference_sq += vr * vr;
        position_max_relative = position_max_relative.max(pd / pr.max(1e-6));
        velocity_max_relative = velocity_max_relative.max(vd / vr.max(1e-9));
    }

    Ok(json!({
        "position_rms_relative_l2": (position_error_sq / position_reference_sq.max(1e-30)).sqrt(),
        "position_max_relative": position_max_relative,
        "velocity_rms_relative_l2": (velocity_error_sq / velocity_reference_sq.max(1e-30)).sqrt(),
        "velocity_max_relative": velocity_max_relative
    }))
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

fn make_bodies(args: &Args) -> Result<Vec<Body>> {
    match args.preset.as_str() {
        "disc" => Ok(make_disc(args.particles, args.seed, 5.0e10, 3.0)?),
        "collision" => Ok(make_collision(args.particles, args.seed)?),
        _ => Err("--preset must be disc or collision".into()),
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("GALAXY Barnes-Hut evolution: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    if !(2..=1_000_000).contains(&args.particles) {
        return Err("--particles must be in 2..=1000000".into());
    }
    if args.preset == "collision" && args.particles < 4 {
        return Err("collision preset requires at least 4 particles".into());
    }
    if !(1..=10_000).contains(&args.steps) {
        return Err("--steps must be in 1..=10000".into());
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
    if args.cpu_reference_limit < 2 || args.cpu_reference_limit > 100_000 {
        return Err("--cpu-reference-limit must be in 2..=100000".into());
    }

    let wall_started = Instant::now();
    let initial_bodies = make_bodies(&args)?;
    let initial_invariants = invariants(&initial_bodies);
    let initial_checksum = state_checksum(&initial_bodies);
    let config = Config {
        theta: args.theta,
        softening_kpc: args.softening_kpc,
        ..Config::default()
    };
    config.validate()?;

    let mut timings = Timings::default();

    let tree_started = Instant::now();
    let initial_tree = build_flat_tree(&initial_bodies, config)?;
    timings.cpu_initial_tree_build = tree_started.elapsed().as_secs_f64();
    let initial_topology = flat_topology_checksum(&initial_tree);

    let pack_started = Instant::now();
    let initial_packed = pack_flat_tree(&initial_bodies, &initial_tree, config)?;
    timings.cpu_pack_f32 += pack_started.elapsed().as_secs_f64();

    let gpu = NbodyGpu::new(args.adapter.as_deref(), args.allow_software)?;
    let (evolving, create_timing) = gpu.create_evolving_state(&initial_bodies)?;
    timings.gpu_state_create_transfer += create_timing.transfer_seconds;
    timings.add_force(gpu.force_into(&evolving, &initial_packed)?);

    let mut previous_topology = initial_topology;
    let mut final_topology = initial_topology;
    let mut topology_changes = 0_u64;

    for _ in 0..args.steps {
        timings.add_kick_drift(gpu.kick_drift(&evolving, args.dt_myr)?);

        let (drifted, drift_readback) = gpu.read_evolving_state(&evolving)?;
        timings.gpu_drift_readback += drift_readback.readback_seconds;

        let rebuild_started = Instant::now();
        let rebuilt = build_flat_tree(&drifted, config)?;
        timings.cpu_tree_rebuild += rebuild_started.elapsed().as_secs_f64();

        final_topology = flat_topology_checksum(&rebuilt);
        if final_topology != previous_topology {
            topology_changes += 1;
        }
        previous_topology = final_topology;

        let pack_started = Instant::now();
        let packed = pack_flat_tree(&drifted, &rebuilt, config)?;
        timings.cpu_pack_f32 += pack_started.elapsed().as_secs_f64();

        timings.add_force(gpu.force_into(&evolving, &packed)?);
        timings.add_final_kick(gpu.final_kick(&evolving, args.dt_myr)?);
    }

    let (final_bodies, final_state_readback) = gpu.read_evolving_state(&evolving)?;
    timings.gpu_final_state_readback += final_state_readback.readback_seconds;
    let (final_acceleration, final_acceleration_readback) =
        gpu.read_evolving_accelerations(&evolving)?;
    timings.gpu_final_acceleration_readback += final_acceleration_readback.readback_seconds;

    let direct_probe_count = args.direct_probes.min(args.particles);
    let direct_started = Instant::now();
    let direct_error = probe_error(
        &final_bodies,
        &final_acceleration,
        config,
        direct_probe_count,
    )?;
    timings.direct_probe_reference = direct_started.elapsed().as_secs_f64();
    if direct_error.rms_relative >= 0.04 || direct_error.max_relative >= 0.30 {
        return Err(format!(
            "final GPU force error too large: rms={} max={}",
            direct_error.rms_relative, direct_error.max_relative
        )
        .into());
    }

    let cpu_reference = if args.particles <= args.cpu_reference_limit {
        let started = Instant::now();
        let reference =
            cpu_flat_evolve(initial_bodies.clone(), args.steps, args.dt_myr, config)?;
        timings.cpu_full_reference = started.elapsed().as_secs_f64();
        let error = state_error(&final_bodies, &reference)?;
        let position_rms = error["position_rms_relative_l2"].as_f64().unwrap();
        let position_max = error["position_max_relative"].as_f64().unwrap();
        let velocity_rms = error["velocity_rms_relative_l2"].as_f64().unwrap();
        let velocity_max = error["velocity_max_relative"].as_f64().unwrap();
        if position_rms >= 0.02
            || position_max >= 0.20
            || velocity_rms >= 0.02
            || velocity_max >= 0.20
        {
            return Err(format!(
                "GPU evolution diverged from flat f64 reference: pos_rms={position_rms} pos_max={position_max} vel_rms={velocity_rms} vel_max={velocity_max}"
            )
            .into());
        }
        json!({
            "status": "executed",
            "particle_limit": args.cpu_reference_limit,
            "state_error": error,
            "final_checksum_fnv_mix64": format!("{:016x}", state_checksum(&reference))
        })
    } else {
        json!({
            "status": "skipped-particle-limit",
            "particle_limit": args.cpu_reference_limit,
            "reason": "full multi-step flat-CPU trajectory reference is bounded so large GPU runs are not dominated by CPU verification"
        })
    };

    let final_invariants = invariants(&final_bodies);
    let final_checksum = state_checksum(&final_bodies);
    let force_solves = args.steps + 1;
    let wall_seconds = wall_started.elapsed().as_secs_f64();

    let receipt = json!({
        "schema": "galaxy.barnes-hut-gpu-evolution-receipt.v1",
        "status": "complete",
        "phase": "BH-2B2",
        "preset": args.preset,
        "particles": args.particles,
        "steps": args.steps,
        "dt_myr": args.dt_myr,
        "simulated_time_myr": args.dt_myr * args.steps as f64,
        "seed": args.seed,
        "theta": args.theta,
        "softening_kpc": args.softening_kpc,
        "gpu": gpu.info(),
        "persistent_gpu_state_bytes": gpu.evolving_buffer_bytes(&evolving),
        "force_solves": force_solves,
        "host_tree_rebuilds": args.steps,
        "topology": {
            "initial_checksum_fnv_mix64": format!("{initial_topology:016x}"),
            "final_checksum_fnv_mix64": format!("{final_topology:016x}"),
            "rebuilds_with_changed_topology": topology_changes
        },
        "state": {
            "initial_checksum_fnv_mix64": format!("{initial_checksum:016x}"),
            "final_checksum_fnv_mix64": format!("{final_checksum:016x}")
        },
        "final_force_direct_probe": {
            "count": direct_probe_count,
            "rms_relative_error": direct_error.rms_relative,
            "max_relative_error": direct_error.max_relative
        },
        "cpu_full_trajectory_reference": cpu_reference,
        "invariants": invariant_json(initial_invariants, final_invariants),
        "timings_seconds": {
            "cpu_initial_tree_build": timings.cpu_initial_tree_build,
            "cpu_tree_rebuild_total": timings.cpu_tree_rebuild,
            "cpu_pack_f32_total": timings.cpu_pack_f32,
            "gpu_state_create_transfer": timings.gpu_state_create_transfer,
            "gpu_force_transfer_total": timings.gpu_force_transfer,
            "gpu_force_dispatch_total": timings.gpu_force_dispatch,
            "gpu_kick_drift_uniform_transfer_total": timings.gpu_kick_drift_transfer,
            "gpu_kick_drift_dispatch_total": timings.gpu_kick_drift_dispatch,
            "gpu_drift_readback_total": timings.gpu_drift_readback,
            "gpu_final_kick_uniform_transfer_total": timings.gpu_final_kick_transfer,
            "gpu_final_kick_dispatch_total": timings.gpu_final_kick_dispatch,
            "gpu_final_state_readback": timings.gpu_final_state_readback,
            "gpu_final_acceleration_readback": timings.gpu_final_acceleration_readback,
            "direct_probe_reference": timings.direct_probe_reference,
            "cpu_full_trajectory_reference": timings.cpu_full_reference,
            "total_wall": wall_seconds
        },
        "scope": "persistent f32 GPU state; GPU force, kick/drift and final-kick execution; host readback only at drift rebuild boundaries plus final evidence; CPU rebuild of BH #2A flat tree; no GPU tree construction or host-independent performance claim"
    });

    if let Some(path) = args.receipt {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serde_json::to_vec_pretty(&receipt)?)?;
    }
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}
