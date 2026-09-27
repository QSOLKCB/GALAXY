// SPDX-License-Identifier: Apache-2.0
use clap::Parser;
use galaxy_nbody::{
    flat::{build_flat_tree, flat_accelerations_from_tree},
    make_disc, probe_error, Accel, Config,
};
use galaxy_runtime::{
    config::Result,
    nbody_gpu::{pack_flat_tree, NbodyGpu},
};
use serde_json::json;
use std::{fs, path::PathBuf, time::Instant};

#[derive(Parser, Debug)]
#[command(about = "Validate BH #2B1 packed Barnes-Hut traversal on the native GPU runtime")]
struct Args {
    #[arg(long, default_value_t = 256)]
    particles: usize,
    #[arg(long, default_value_t = 303)]
    seed: u64,
    #[arg(long, default_value_t = 0.5)]
    theta: f64,
    #[arg(long, default_value_t = 12)]
    direct_probes: usize,
    #[arg(long)]
    adapter: Option<String>,
    #[arg(long)]
    allow_software: bool,
    #[arg(long)]
    receipt: Option<PathBuf>,
}

fn error_summary(approximate: &[Accel], reference: &[Accel]) -> Result<(f64, f64)> {
    if approximate.len() != reference.len() || approximate.is_empty() {
        return Err("acceleration arrays must be non-empty and equal length".into());
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

fn main() {
    if let Err(error) = run() {
        eprintln!("GALAXY Barnes-Hut GPU: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    if !(2..=1_000_000).contains(&args.particles) {
        return Err("--particles must be in 2..=1000000".into());
    }
    if !args.theta.is_finite() || !(0.0..=2.0).contains(&args.theta) {
        return Err("--theta must be finite and in [0, 2]".into());
    }
    if !(1..=64).contains(&args.direct_probes) {
        return Err("--direct-probes must be in 1..=64".into());
    }

    let bodies = make_disc(args.particles, args.seed, 5.0e10, 3.0)?;
    let config = Config {
        theta: args.theta,
        ..Config::default()
    };

    let build_started = Instant::now();
    let tree = build_flat_tree(&bodies, config)?;
    let cpu_tree_build_seconds = build_started.elapsed().as_secs_f64();

    let pack_started = Instant::now();
    let packed = pack_flat_tree(&bodies, &tree, config)?;
    let cpu_pack_seconds = pack_started.elapsed().as_secs_f64();

    let cpu_started = Instant::now();
    let (flat_reference, _) = flat_accelerations_from_tree(&bodies, &tree, config)?;
    let cpu_flat_traversal_seconds = cpu_started.elapsed().as_secs_f64();

    let gpu = NbodyGpu::new(args.adapter.as_deref(), args.allow_software)?;
    let result = gpu.traverse(&packed)?;
    let (gpu_flat_rms, gpu_flat_max) = error_summary(&result.accelerations, &flat_reference)?;

    let direct_probe_count = args.direct_probes.min(args.particles);
    let direct_started = Instant::now();
    let direct_probe = probe_error(
        &bodies,
        &result.accelerations,
        config,
        direct_probe_count,
    )?;
    let direct_probe_seconds = direct_started.elapsed().as_secs_f64();
    let gpu_direct_rms = direct_probe.rms_relative;
    let gpu_direct_max = direct_probe.max_relative;

    // The direct-force gates preserve the BH #1 accuracy contract with a small
    // allowance for the deliberate f64 -> f32 transfer boundary.
    if gpu_direct_rms >= 0.04 || gpu_direct_max >= 0.30 {
        return Err(format!(
            "GPU Barnes-Hut direct-force error too large: rms={gpu_direct_rms} max={gpu_direct_max}"
        )
        .into());
    }
    if gpu_flat_rms >= 0.01 || gpu_flat_max >= 0.10 {
        return Err(format!(
            "GPU traversal diverged from the BH #2A flat CPU oracle: rms={gpu_flat_rms} max={gpu_flat_max}"
        )
        .into());
    }

    let receipt = json!({
        "schema": "galaxy.barnes-hut-gpu-traversal-receipt.v1",
        "status": "complete",
        "phase": "BH-2B1",
        "particles": args.particles,
        "seed": args.seed,
        "theta": args.theta,
        "gpu": result.gpu_info,
        "packed_transfer_bytes": result.packed_bytes,
        "timings_seconds": {
            "cpu_tree_build": cpu_tree_build_seconds,
            "cpu_pack_f32": cpu_pack_seconds,
            "cpu_flat_traversal_reference": cpu_flat_traversal_seconds,
            "direct_probe_reference": direct_probe_seconds,
            "gpu_transfer": result.transfer_seconds,
            "gpu_dispatch": result.dispatch_seconds,
            "gpu_readback": result.readback_seconds
        },
        "direct_probe_count": direct_probe_count,
        "errors": {
            "gpu_vs_flat_cpu_rms_relative": gpu_flat_rms,
            "gpu_vs_flat_cpu_max_relative": gpu_flat_max,
            "gpu_vs_direct_rms_relative": gpu_direct_rms,
            "gpu_vs_direct_max_relative": gpu_direct_max
        },
        "record_bytes": {
            "settings": 32,
            "body": 32,
            "entry": 16,
            "cell": 64,
            "acceleration": 16
        },
        "scope": "CPU-built BH #2A flat tree packed to f32/u32 and traversed on GPU; full GPU-vs-flat comparison plus bounded direct-force probes; no GPU tree construction, evolving leapfrog state, or host-independent performance claim"
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
