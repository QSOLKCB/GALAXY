// SPDX-License-Identifier: Apache-2.0
//! Experimental GALAXY CPU oversubscription probe.
//!
//! This binary deliberately bypasses only the canonical available_parallelism
//! worker cap so host oversubscription can be measured without changing the
//! production galaxy-cpu contract. Numerical work, partitioning, timing order,
//! deterministic reduction, and checksum validation are reused from the
//! canonical runtime.

mod canonical {
    include!(concat!(env!("OUT_DIR"), "/legacy_runtime.inc.rs"));

    const OVERSUB_RECEIPT_SCHEMA: &str = "galaxy.cpu-oversubscription-probe.v1";

    fn oversubscription_workers(
        work_items: usize,
        requested_workers: usize,
    ) -> Result<(usize, usize), String> {
        if requested_workers == 0 {
            return Err("workers must be greater than zero".into());
        }
        if work_items == 0 {
            return Ok((available_parallelism(), 0));
        }
        let available = available_parallelism();
        let effective = requested_workers
            .min(work_items)
            .min(MAX_WORKERS)
            .max(1);
        Ok((available, effective))
    }

    fn measure_backends_oversubscribed(
        particles: &[Particle],
        config: &Config,
        lut: &Lut,
    ) -> Result<(BackendEvidence, BackendEvidence, usize, usize), String> {
        let (available, effective) =
            oversubscription_workers(particles.len(), config.requested_workers)?;

        for backend in [Backend::Float, Backend::Lut] {
            let warm_scalar =
                black_box(execute_scalar(particles, config.frames, backend, lut));
            let warm_parallel = black_box(execute_parallel_fixed(
                particles,
                config.frames,
                backend,
                lut,
                effective,
            )?);
            if warm_scalar != warm_parallel {
                return Err(format!("{} warm-up checksum mismatch", backend.name()));
            }
        }

        let mut float_samples = BackendSamples::new(config.repeats);
        let mut lut_samples = BackendSamples::new(config.repeats);

        for repeat in 0..config.repeats {
            if repeat % 2 == 0 {
                measure_trial(
                    particles,
                    config,
                    Backend::Float,
                    lut,
                    effective,
                    false,
                    &mut float_samples,
                )?;
                measure_trial(
                    particles,
                    config,
                    Backend::Lut,
                    lut,
                    effective,
                    false,
                    &mut lut_samples,
                )?;
                measure_trial(
                    particles,
                    config,
                    Backend::Float,
                    lut,
                    effective,
                    true,
                    &mut float_samples,
                )?;
                measure_trial(
                    particles,
                    config,
                    Backend::Lut,
                    lut,
                    effective,
                    true,
                    &mut lut_samples,
                )?;
            } else {
                measure_trial(
                    particles,
                    config,
                    Backend::Lut,
                    lut,
                    effective,
                    true,
                    &mut lut_samples,
                )?;
                measure_trial(
                    particles,
                    config,
                    Backend::Float,
                    lut,
                    effective,
                    true,
                    &mut float_samples,
                )?;
                measure_trial(
                    particles,
                    config,
                    Backend::Lut,
                    lut,
                    effective,
                    false,
                    &mut lut_samples,
                )?;
                measure_trial(
                    particles,
                    config,
                    Backend::Float,
                    lut,
                    effective,
                    false,
                    &mut float_samples,
                )?;
            }
        }

        let float =
            finish_backend_evidence(float_samples, Backend::Float, available, effective)?;
        let lut_evidence =
            finish_backend_evidence(lut_samples, Backend::Lut, available, effective)?;
        Ok((float, lut_evidence, available, effective))
    }

    fn oversubscription_receipt_json(
        config: &Config,
        available: usize,
        effective: usize,
        lut_sampled_error_q30: i64,
        float: BackendEvidence,
        lut: BackendEvidence,
    ) -> String {
        let oversubscribed = effective > available;
        format!(
            "{{\n  \"schema\": \"{OVERSUB_RECEIPT_SCHEMA}\",\n  \"runtime\": \"galaxy-cpu-oversubscription-probe\",\n  \"execution_mode\": \"experimental-software-worker-oversubscription\",\n  \"canonical_runtime_modified\": false,\n  \"canonical_worker_cap_bypassed\": true,\n  \"architecture\": \"{}\",\n  \"os\": \"{}\",\n  \"addressing\": \"{ADDRESSING}\",\n  \"logical_population\": \"{}\",\n  \"resident_particles\": {},\n  \"frames\": {},\n  \"repeats\": {},\n  \"seed\": {},\n  \"requested_workers\": {},\n  \"available_parallelism\": {},\n  \"effective_workers\": {},\n  \"oversubscribed\": {},\n  \"lut_entries\": {},\n  \"lut_error_sample_count\": {},\n  \"lut_sampled_max_abs_q30_error\": {},\n  \"float\": {{\n    \"scalar_median_ns\": {},\n    \"parallel_median_ns\": {},\n    \"scalar_checksum\": \"{:016x}\",\n    \"parallel_checksum\": \"{:016x}\",\n    \"checksum_match\": {},\n    \"measured_speedup\": {:.9}\n  }},\n  \"bam_lut\": {{\n    \"scalar_median_ns\": {},\n    \"parallel_median_ns\": {},\n    \"scalar_checksum\": \"{:016x}\",\n    \"parallel_checksum\": \"{:016x}\",\n    \"checksum_match\": {},\n    \"measured_speedup\": {:.9}\n  }},\n  \"claim_boundary\": \"Experimental host-specific oversubscription evidence. effective_workers may exceed available_parallelism by design. This probe does not alter the canonical galaxy-cpu worker cap and does not imply that software-worker count corresponds to physical cores, address bits, or a universal optimum.\"\n}}\n",
            env::consts::ARCH,
            env::consts::OS,
            config.logical,
            config.resident,
            config.frames,
            config.repeats,
            config.seed,
            config.requested_workers,
            available,
            effective,
            json_bool(oversubscribed),
            LUT_SIZE,
            LUT_ERROR_SAMPLE_COUNT,
            lut_sampled_error_q30,
            float.scalar.median_ns,
            float.parallel.median_ns,
            float.scalar.checksum,
            float.parallel.checksum,
            json_bool(float.checksum_match),
            float.speedup,
            lut.scalar.median_ns,
            lut.parallel.median_ns,
            lut.scalar.checksum,
            lut.parallel.checksum,
            json_bool(lut.checksum_match),
            lut.speedup,
        )
    }

    pub fn run_oversubscription(args: &[String]) -> Result<(), String> {
        if help_requested(args) {
            print!(
                "Usage:\n  galaxy-cpu-oversubscription bench [--logical U64] [--resident N] [--frames N] [--workers N] [--repeats N] [--seed U32] [--receipt PATH]\n\nExperimental semantics:\n  effective_workers=min(requested_workers,resident_particles,256)\n  available_parallelism is recorded but deliberately does not cap workers.\n"
            );
            return Ok(());
        }

        let bench_args = match args.first().map(String::as_str) {
            Some("bench") | Some("run") => &args[1..],
            Some(command) => return Err(format!("unknown command: {command}")),
            None => return Err("missing command; use bench".into()),
        };

        let config = parse_config(bench_args)?;
        let build_started = Instant::now();
        let particles = build_particles(config.logical, config.resident, config.seed)?;
        let resident_build_ns = build_started.elapsed().as_nanos();
        let lut = Lut::build();
        let lut_sampled_error_q30 = lut_sampled_error(&lut);

        let (float, lut_evidence, available, effective) =
            measure_backends_oversubscribed(&particles, &config, &lut)?;

        println!("galaxy_cpu_oversubscription_probe=v1");
        println!("arch={}", env::consts::ARCH);
        println!("os={}", env::consts::OS);
        println!("logical_population={}", config.logical);
        println!("resident_particles={}", config.resident);
        println!("frames={}", config.frames);
        println!("requested_workers={}", config.requested_workers);
        println!("available_parallelism={available}");
        println!("effective_workers={effective}");
        println!("oversubscribed={}", effective > available);
        println!("repeats={}", config.repeats);
        println!("addressing={ADDRESSING}");
        println!("resident_build_ns={resident_build_ns}");
        println!("backend_timing_schedule=interleaved-alternating-v1");
        print_timing(
            "float_scalar",
            float.scalar,
            config.resident as u128 * config.frames as u128,
        );
        print_timing(
            "float_parallel",
            float.parallel,
            config.resident as u128 * config.frames as u128,
        );
        println!("float_parallel_speedup={:.9}", float.speedup);
        print_timing(
            "bam_lut_scalar",
            lut_evidence.scalar,
            config.resident as u128 * config.frames as u128,
        );
        print_timing(
            "bam_lut_parallel",
            lut_evidence.parallel,
            config.resident as u128 * config.frames as u128,
        );
        println!("bam_lut_parallel_speedup={:.9}", lut_evidence.speedup);

        if let Some(path) = &config.receipt {
            let receipt = oversubscription_receipt_json(
                &config,
                available,
                effective,
                lut_sampled_error_q30,
                float,
                lut_evidence,
            );
            write_receipt(path, &receipt)?;
            println!("receipt={}", path.display());
        }

        Ok(())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = canonical::run_oversubscription(&args) {
        eprintln!("galaxy-cpu-oversubscription: {error}");
        std::process::exit(2);
    }
}
