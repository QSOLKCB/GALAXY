// SPDX-License-Identifier: Apache-2.0
use galaxy_runtime::{config::*, output, reference, verify};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn original_uff_references_match_native_equations() {
    let report = verify::run(None).unwrap();
    assert_eq!(report["uff_python_predictions"], 195);
    assert_eq!(report["compact_python_predictions"], 45);
    assert_eq!(report["gpu_checked"], false);
}
#[test]
fn job_contract_rejects_unknown_fields_and_impossible_budgets() {
    let job: Job =
        serde_json::from_value(json!({"schema_version":1,"task":{"kind":"spin"}})).unwrap();
    job.validate().unwrap();
    assert!(serde_json::from_value::<Job>(
        json!({"schema_version":1,"task":{"kind":"spin","particels":123}})
    )
    .is_err());
    assert!(serde_json::from_value::<Job>(
        json!({"schema_version":1,"task":{"kind":"spin","physics":{"model":"legacy"}}})
    )
    .is_err());
    for mut s in [
        Spin {
            particles: 0,
            ..Spin::default()
        },
        Spin {
            particles: MAX_PARTICLES + 1,
            ..Spin::default()
        },
        Spin {
            radial_kick_kms: 20.0,
            ..Spin::default()
        },
        Spin {
            dt_myr: f64::NAN,
            ..Spin::default()
        },
        Spin {
            steps: 100000,
            snapshot_every: 1,
            ..Spin::default()
        },
    ] {
        assert!(s.validate().is_err());
        s = Spin::default();
        assert!(s.validate().is_ok());
    }
    let c = Curves {
        radii: u32::MAX,
        sweep: Some(Sweep {
            parameter: SweepParameter::UffBeta,
            min: 0.0,
            max: 1.0,
            count: u32::MAX,
        }),
        ..Curves::default()
    };
    assert!(c.validate().is_err());
    assert!(Compact {
        mass_min_msun: 0.0,
        ..Compact::default()
    }
    .validate()
    .is_err());
}
#[test]
fn shipped_jobs_validate_and_carry_actual_particle_counts() {
    for name in [
        "spin-local",
        "spin-vast",
        "perturbed-orbits",
        "model-comparison",
        "uff-sweep",
        "compact-objects",
    ] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("jobs/{name}.json"));
        let job: Job = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        job.validate().unwrap();
    }
    assert_eq!(std::mem::size_of::<reference::Particle>(), 32);
    assert_eq!(
        reference::sample_index(65535, 65536, MAX_PARTICLES),
        MAX_PARTICLES - 128
    );
    assert_eq!(reference::sample_index(0, 65536, MAX_PARTICLES), 0);
}
#[test]
fn circular_motion_preserves_radius_and_reverses_direction() {
    let s = Spin {
        particles: 513,
        steps: 800,
        ..Spin::default()
    };
    let initial = reference::initialize(&s);
    let mut evolved = initial.clone();
    reference::advance(&mut evolved, &s, 800, 800);
    for (a, b) in evolved.iter().zip(&initial) {
        assert!((a.state[0].hypot(a.state[1]) - b.orbit[0]).abs() < 2e-6);
        assert_eq!(a.orbit, b.orbit);
    }
    let reversed = reference::initialize(&Spin { direction: -1, ..s });
    assert!(initial
        .iter()
        .zip(reversed)
        .all(|(a, b)| a.state[2] == -b.state[2] && a.state[3] == -b.state[3]));
}
#[test]
fn leapfrog_converges_and_preserves_angular_momentum() {
    let mut s = Spin {
        particles: 1,
        integrator: Integrator::Leapfrog,
        dt_myr: 1.0,
        steps: 100,
        radial_kick_kms: 10.0,
        ..Spin::default()
    };
    s.physics.black_hole_million = 1000.0;
    let radius = 1.0_f64;
    let softened = (radius * radius + s.softening_kpc.powi(2)).sqrt();
    let speed =
        reference::velocity(softened, &s.physics) * reference::KMS_TO_KPC_MYR * radius / softened;
    let initial = vec![reference::Particle {
        orbit: [radius as f32, 0.0, 0.0, (speed / radius) as f32],
        state: [
            radius as f32,
            0.0,
            (s.radial_kick_kms * reference::KMS_TO_KPC_MYR) as f32,
            speed as f32,
        ],
    }];
    let integrate = |dt: f64, steps: u32| {
        let mut p = initial.clone();
        let config = Spin {
            dt_myr: dt,
            steps,
            ..s.clone()
        };
        reference::advance(&mut p, &config, steps, steps);
        p[0]
    };
    let coarse = integrate(1.0, 100);
    let medium = integrate(0.5, 200);
    let fine = integrate(0.0625, 1600);
    let error = |p: reference::Particle| {
        p.state
            .iter()
            .zip(fine.state)
            .map(|(a, b)| (*a as f64 - b as f64).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    assert!(
        error(medium) < 0.6 * error(coarse),
        "{} vs {}",
        error(medium),
        error(coarse)
    );
    let lz = |p: reference::Particle| {
        p.state[0] as f64 * p.state[3] as f64 - p.state[1] as f64 * p.state[2] as f64
    };
    assert!(((lz(fine) - lz(initial[0])) / lz(initial[0])).abs() < 2e-5);
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "galaxy-runtime-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn cli_writes_complete_results_and_never_overwrites_existing_runs() {
    let temp = Scratch::new();
    for (name, task) in [
        (
            "spin",
            json!({"kind":"spin","particles":257,"snapshot_limit":129,"steps":4,"snapshot_every":2,"image_size":128}),
        ),
        ("curves", json!({"kind":"curves","radii":3})),
        ("compact", json!({"kind":"compact","masses":2,"spins":3})),
    ] {
        let job = temp.0.join(format!("{name}.json"));
        let out = temp.0.join(name);
        fs::write(
            &job,
            serde_json::to_vec(&json!({"schema_version":1,"task":task})).unwrap(),
        )
        .unwrap();
        let run = || {
            Command::new(env!("CARGO_BIN_EXE_galaxy-runtime"))
                .args(["run", "--cpu", "--job"])
                .arg(&job)
                .arg("--output")
                .arg(&out)
                .output()
                .unwrap()
        };
        let result = run();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let receipt: Value =
            serde_json::from_slice(&fs::read(out.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(receipt["status"], "complete");
        assert_eq!(receipt["results"]["adapter"]["backend"], "CPU");
        for artifact in receipt["artifacts"].as_array().unwrap() {
            assert_eq!(
                output::sha256(&fs::read(out.join(artifact["file"].as_str().unwrap())).unwrap()),
                artifact["sha256"]
            );
        }
        if name == "spin" {
            assert_eq!(
                receipt["results"]["spin"]["frames"]
                    .as_array()
                    .unwrap()
                    .len(),
                3
            );
            assert_eq!(
                fs::read_to_string(out.join("frame-000004.csv"))
                    .unwrap()
                    .lines()
                    .count(),
                130
            );
            let decoder = png::Decoder::new(File::open(out.join("frame-000004.png")).unwrap());
            assert_eq!(decoder.read_info().unwrap().info().width, 128);
        }
        let before = fs::read(out.join("receipt.json")).unwrap();
        assert!(!run().status.success());
        assert_eq!(before, fs::read(out.join("receipt.json")).unwrap());
    }
}
use std::fs::File;
