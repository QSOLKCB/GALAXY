// SPDX-License-Identifier: Apache-2.0
//! Production command dispatcher for GALAXY's native CPU runtime.
//!
//! The canonical AoS runtime remains the default command surface. The worker-local
//! SoA path is integrated behind explicit `bench-soa` / `verify-soa` commands so
//! callers must opt in while the established path remains the oracle and fallback.

use std::env;

mod legacy {
    include!("../main.rs");

    pub fn run() {
        main();
    }

    pub fn usage_text() -> &'static str {
        usage()
    }
}

mod integrated_soa {
    include!("worker_soa_probe.rs");

    const INTEGRATED_RECEIPT_SCHEMA: &str = "galaxy.cpu-runtime-soa-receipt.v1";
    const INTEGRATED_DEFAULT_TILE: usize = 1_024;

    pub fn usage_text() -> &'static str {
        "Usage:\n  galaxy-cpu verify-soa [--workers N] [--tile N]\n  galaxy-cpu bench-soa [--logical U64] [--resident N] [--frames N] [--workers N] [--tile N] [--repeats N] [--seed U32] [--receipt PATH]\n\nGuarded SoA defaults:\n  logical=18446744073709551615 resident=262144 frames=8 repeats=3 tile=1024 seed=303\n  workers=min(std::thread::available_parallelism(), 256)\n\nThe canonical `bench` / `verify` commands remain unchanged and are the default/oracle path.\n"
    }

    fn integrated_args(args: &[String]) -> Result<Vec<String>, String> {
        if args.iter().any(|arg| arg == "--path") {
            return Err("bench-soa selects the worker-local SoA path; --path is not accepted".into());
        }
        let mut filtered = args.to_vec();
        if !args.iter().any(|arg| arg == "--tile") {
            filtered.push("--tile".into());
            filtered.push(INTEGRATED_DEFAULT_TILE.to_string());
        }
        Ok(filtered)
    }

    fn integrated_verify_args(args: &[String]) -> Vec<String> {
        let mut filtered = args.to_vec();
        if !args.iter().any(|arg| arg == "--tile") {
            filtered.push("--tile".into());
            filtered.push(INTEGRATED_DEFAULT_TILE.to_string());
        }
        filtered
    }

    fn integrated_receipt_json(config: &Config, measurement: Measurement) -> String {
        let compact_field_bytes = 5 * size_of::<u32>();
        let scratch_bytes = 2 * size_of::<u32>() + size_of::<u64>();
        let worker_tile_capacity_particles = soa_worker_tile_capacity_particles(
            config.resident,
            measurement.effective_workers,
            config.tile_particles,
        );
        let worker_tile_capacity_bytes = (compact_field_bytes + scratch_bytes)
            .saturating_mul(worker_tile_capacity_particles);
        format!(
            "{{\n  \"schema\": \"{INTEGRATED_RECEIPT_SCHEMA}\",\n  \"runtime\": \"galaxy-cpu\",\n  \"execution_mode\": \"worker-local-soa-guarded\",\n  \"guarded_opt_in\": true,\n  \"canonical_fallback\": \"bench\",\n  \"architecture\": \"{}\",\n  \"os\": \"{}\",\n  \"addressing\": \"{ADDRESSING}\",\n  \"logical_population\": \"{}\",\n  \"resident_particles\": {},\n  \"frames\": {},\n  \"repeats\": {},\n  \"seed\": {},\n  \"requested_workers\": {},\n  \"available_parallelism\": {},\n  \"effective_workers\": {},\n  \"tile_particles\": {},\n  \"soa_compact_field_bytes_per_particle\": {},\n  \"soa_scratch_bytes_per_particle\": {},\n  \"soa_worker_tile_capacity_bytes\": {},\n  \"best_ns\": {},\n  \"median_ns\": {},\n  \"checksum\": \"{:016x}\",\n  \"peak_rss_kib\": {},\n  \"resident_generation_in_timed_region\": true,\n  \"timing_scope\": \"resident-generation-plus-bam-lut-projection-plus-contribution-plus-worker-reduction\",\n  \"claim_boundary\": \"Guarded opt-in worker-local SoA execution integrated into galaxy-cpu. The canonical AoS `bench` path remains the default oracle/fallback; this receipt is host-specific performance evidence, not a universal speedup claim. RSS is Linux VmHWM when available, otherwise null.\"\n}}\n",
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
            compact_field_bytes,
            scratch_bytes,
            worker_tile_capacity_bytes,
            measurement.timing.best_ns,
            measurement.timing.median_ns,
            measurement.timing.checksum,
            json_optional_u64(measurement.peak_rss_kib),
        )
    }

    pub fn run_bench(args: &[String]) -> Result<(), String> {
        let filtered = integrated_args(args)?;
        let mut config = parse_config(&filtered)?;
        config.path = ExecutionPath::WorkerSoa;

        let lut = Lut::build();
        let measurement = measure(&config, &lut)?;
        println!("galaxy_cpu_runtime=v2");
        println!("execution_mode=worker-local-soa-guarded");
        println!("canonical_fallback=bench");
        println!("logical_population={}", config.logical);
        println!("resident_particles={}", config.resident);
        println!("frames={}", config.frames);
        println!("requested_workers={}", config.requested_workers);
        println!("available_parallelism={}", measurement.available_parallelism);
        println!("effective_workers={}", measurement.effective_workers);
        println!("tile_particles={}", config.tile_particles);
        println!("best_ns={}", measurement.timing.best_ns);
        println!("median_ns={}", measurement.timing.median_ns);
        println!("checksum={:016x}", measurement.timing.checksum);
        match measurement.peak_rss_kib {
            Some(value) => println!("peak_rss_kib={value}"),
            None => println!("peak_rss_kib=unavailable"),
        }
        if let Some(path) = &config.receipt {
            write_receipt(path, &integrated_receipt_json(&config, measurement))?;
            println!("receipt={}", path.display());
        }
        Ok(())
    }

    pub fn run_verify(args: &[String]) -> Result<(), String> {
        let filtered = integrated_verify_args(args);
        let (workers, tile) = parse_verify(&filtered)?;
        run_verify(workers, tile)?;
        println!("GALAXY guarded worker-local SoA integration verification passed");
        Ok(())
    }
}

fn combined_usage() -> String {
    format!(
        "{}\nGuarded worker-local SoA integration:\n{}",
        legacy::usage_text(),
        integrated_soa::usage_text()
    )
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("bench-soa") | Some("run-soa") if args[1..].iter().any(|arg| arg == "--help" || arg == "-h") => {
            print!("{}", integrated_soa::usage_text());
            Ok(())
        }
        Some("bench-soa") | Some("run-soa") => integrated_soa::run_bench(&args[1..]),
        Some("verify-soa") if args[1..].iter().any(|arg| arg == "--help" || arg == "-h") => {
            print!("{}", integrated_soa::usage_text());
            Ok(())
        }
        Some("verify-soa") => integrated_soa::run_verify(&args[1..]),
        Some("--help") | Some("-h") | None => {
            print!("{}", combined_usage());
            Ok(())
        }
        _ => {
            legacy::run();
            return;
        }
    };

    if let Err(error) = result {
        eprintln!("galaxy-cpu: {error}");
        std::process::exit(2);
    }
}
