// SPDX-License-Identifier: Apache-2.0
use clap::{Args, Parser, Subcommand};
use galaxy_runtime::{
    config::{Integrator, Job, Result, Task},
    gpu::{self, Case, Gpu},
    output, reference, verify,
};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Parser)]
#[command(version, about = "Headless Rust compute jobs for GALAXY / UFF")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Args)]
struct Backend {
    #[arg(long,conflicts_with_all=["adapter","allow_software"],help="Use the explicit CPU reference")]
    cpu: bool,
    #[arg(
        long,
        help = "Adapter index from devices, or a case-insensitive name substring"
    )]
    adapter: Option<String>,
    #[arg(
        long,
        help = "Permit software Vulkan for validation; recorded in the receipt"
    )]
    allow_software: bool,
}
#[derive(Subcommand)]
enum Command {
    /// List available native compute adapters and storage limits.
    Devices,
    /// Validate a job and print its resolved defaults without starting a GPU.
    Validate {
        #[arg(long)]
        job: PathBuf,
    },
    /// Check native/GPU equations against original UFF Python fixtures.
    Verify {
        #[command(flatten)]
        backend: Backend,
    },
    /// Execute a job into a new output directory.
    Run {
        #[arg(long)]
        job: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        backend: Backend,
    },
}
fn load_job(path: &Path) -> Result<Job> {
    if fs::metadata(path)?.len() > 65536 {
        return Err("Job JSON must be at most 64 KiB".into());
    }
    let job: Job = serde_json::from_slice(&fs::read(path)?)?;
    job.validate()?;
    Ok(job)
}
fn backend(options: &Backend) -> Result<Option<Gpu>> {
    if options.cpu {
        Ok(None)
    } else {
        Ok(Some(Gpu::new(
            options.adapter.as_deref(),
            options.allow_software,
        )?))
    }
}
fn execute(job: &Job, directory: &Path, options: &Backend) -> Result<Value> {
    let gpu = backend(options)?;
    let adapter = gpu.as_ref().map_or(
        json!({"name":"native CPU reference","backend":"CPU","software":true}),
        |g| g.info.clone(),
    );
    eprintln!("Engine: {} ({})", adapter["name"], adapter["backend"]);
    let started = Instant::now();
    let mut details = json!({"adapter":adapter,"arithmetic":if gpu.is_some() {"float32 GPU kernels; float64 CPU LQG scale diagnostics"} else {"float64 CPU equations; float32 stored particle state"}});
    match &job.task {
        Task::Spin(s) => {
            let mut cpu = if gpu.is_none() {
                reference::initialize(s)
            } else {
                Vec::new()
            };
            let field = gpu.as_ref().map(|g| g.initialize(s)).transpose()?;
            let mut step = 0;
            let mut frames = Vec::new();
            let mut updates = 0_u64;
            let mut compute_seconds = 0.0;
            loop {
                let points = if let (Some(g), Some(f)) = (&gpu, &field) {
                    g.sample(f, s)?
                } else {
                    reference::sample(&cpu, s.sample_count())
                };
                let frame = output::snapshot(directory, s, step, &points)?;
                eprintln!(
                    "step {step}/{} · {:.2} Myr · {} exported of {} simulated",
                    s.steps,
                    step as f64 * s.dt_myr,
                    points.len(),
                    s.particles
                );
                frames.push(frame);
                if step == s.steps {
                    break;
                }
                let count = if s.snapshot_every == 0 {
                    s.steps - step
                } else {
                    s.snapshot_every.min(s.steps - step)
                };
                step += count;
                let start = Instant::now();
                if let (Some(g), Some(f)) = (&gpu, &field) {
                    g.advance(f, s, count, step);
                } else {
                    reference::advance(&mut cpu, s, count, step);
                }
                compute_seconds += start.elapsed().as_secs_f64();
                updates += s.particles as u64
                    * if s.integrator == Integrator::Circular {
                        1
                    } else {
                        count as u64
                    };
            }
            output::viewer(directory, &frames, s.particles)?;
            details["spin"] = json!({"simulated_particles":s.particles,"resident_particle_bytes":s.particles as u64*32,
                "snapshot_limit":s.sample_count(),"simulated_time_myr":s.steps as f64*s.dt_myr,"particle_updates":updates,
                "synchronized_compute_wall_seconds":compute_seconds,"frames":frames,
                "force_model":if s.integrator==Integrator::Circular {"prescribed UFF circular speed"} else {"planar fixed UFF potential; softened radial force; static authored height"}});
        }
        Task::Curves(c) => {
            let cases = c.cases();
            let values: Vec<f64> = if let Some(g) = &gpu {
                g.evaluate(
                    "curves",
                    &cases
                        .iter()
                        .map(|(r, _, p)| Case::curve(*r, p))
                        .collect::<Vec<_>>(),
                )?
                .iter()
                .map(|p| p.orbit[0] as f64)
                .collect()
            } else {
                cases
                    .iter()
                    .map(|(r, _, p)| reference::velocity(*r, p))
                    .collect()
            };
            let mut writer = BufWriter::new(File::create(directory.join("rotation-curves.csv"))?);
            writeln!(
                writer,
                "model,sweep_value,radius_kpc,velocity_kms,acceleration_m_s2,orbital_period_myr"
            )?;
            for ((r, sweep, p), v) in cases.iter().zip(values) {
                let sweep = if c.sweep.is_some() {
                    sweep.to_string()
                } else {
                    String::new()
                };
                writeln!(
                    writer,
                    "{},{sweep},{r:.9},{v:.9},{:.12e},{:.9}",
                    p.model.name(),
                    v * v * 1e6 / (r * galaxy_sampler::physics::KPC_TO_M),
                    std::f64::consts::TAU * r / (v * reference::KMS_TO_KPC_MYR)
                )?;
            }
            writer.flush()?;
            details["curve_evaluations"] = json!(cases.len());
        }
        Task::Compact(c) => {
            let cases = c.cases();
            let values: Vec<[f64; 4]> = if let Some(g) = &gpu {
                g.evaluate(
                    "compact",
                    &cases
                        .iter()
                        .map(|(m, s)| Case::compact(*m, *s))
                        .collect::<Vec<_>>(),
                )?
                .iter()
                .map(|p| p.orbit.map(f64::from))
                .collect()
            } else {
                cases
                    .iter()
                    .map(|(m, s)| reference::compact(*m, *s))
                    .collect()
            };
            let mut writer = BufWriter::new(File::create(directory.join("compact-objects.csv"))?);
            writeln!(writer,"mass_msun,spin,gravitational_radius_kpc,horizon_rg,photon_orbit_rg,isco_rg,horizon_kpc,photon_orbit_kpc,isco_kpc,sphere_of_influence_kpc,area_gap_over_isco_squared")?;
            for ((mass, spin), [h, p, i, rg]) in cases.iter().zip(values) {
                let radius = i * rg * galaxy_sampler::physics::KPC_TO_M;
                writeln!(writer,"{mass:.9e},{spin:.9},{rg:.12e},{h:.9},{p:.9},{i:.9},{:.12e},{:.12e},{:.12e},{:.12e},{:.12e}",h*rg,p*rg,i*rg,reference::influence_radius(*mass,c.velocity_dispersion_kms),reference::area_gap()/(radius*radius))?;
            }
            writer.flush()?;
            details["compact"] = json!({"evaluations":cases.len(),"barbero_immirzi":0.2375,"area_gap_m2":reference::area_gap(),
                "area_gap_computation":"float64 CPU scale bookkeeping; no LQG force or effective metric"});
        }
    }
    details["execution_wall_seconds"] = json!(started.elapsed().as_secs_f64());
    Ok(details)
}
fn run(job_path: &Path, directory: &Path, options: &Backend) -> Result<()> {
    let job = load_job(job_path)?;
    if let Some(parent) = directory.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(directory).map_err(|e| {
        format!(
            "Output directory must be new ({}): {e}",
            directory.display()
        )
    })?;
    output::write_json(&directory.join("job.json"), &job)?;
    let provenance: Value = serde_json::from_str(include_str!("../../data/uff/provenance.json"))?;
    let mut receipt = json!({"schema_version":1,"runtime_version":env!("CARGO_PKG_VERSION"),"status":"running",
        "runtime_source_sha256":env!("GALAXY_RUNTIME_SOURCE_SHA256"),"job_sha256":output::sha256(&fs::read(directory.join("job.json"))?),
        "uff_source":provenance,"backend_requested":if options.cpu {"cpu"} else {"gpu"},"allow_software":options.allow_software});
    output::write_json(&directory.join("receipt.json"), &receipt)?;
    match execute(&job, directory, options) {
        Ok(details) => {
            receipt["status"] = json!("complete");
            receipt["results"] = details;
            receipt["artifacts"] = json!(output::artifacts(directory)?);
            output::write_json(&directory.join("receipt.json"), &receipt)?;
        }
        Err(error) => {
            receipt["status"] = json!("failed");
            receipt["error"] = json!(error.to_string());
            output::write_json(&directory.join("receipt.json"), &receipt)?;
            return Err(error);
        }
    }
    println!("Completed: {}", directory.display());
    Ok(())
}
fn main() {
    let result: Result<()> = (|| {
        match Cli::parse().command {
            Command::Devices => println!("{}", serde_json::to_string_pretty(&gpu::devices())?),
            Command::Validate { job } => {
                println!("{}", serde_json::to_string_pretty(&load_job(&job)?)?)
            }
            Command::Verify { backend: options } => {
                let gpu = backend(&options)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&verify::run(gpu.as_ref())?)?
                );
            }
            Command::Run {
                job,
                output,
                backend,
            } => run(&job, &output, &backend)?,
        }
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("GALAXY: {error}");
        std::process::exit(1);
    }
}
