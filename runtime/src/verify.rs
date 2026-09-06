// SPDX-License-Identifier: Apache-2.0
use crate::{
    config::{Integrator, Model, Physics, Result, Spin},
    gpu::{Case, Gpu},
    reference,
};
use serde_json::{json, Value};
fn close(actual: f64, expected: f64, tolerance: f64, label: &str) -> Result<()> {
    let scale = if expected == 0.0 { 1.0 } else { expected.abs() };
    if !actual.is_finite() || (actual - expected).abs() > tolerance * scale {
        return Err(format!(
            "{label}: {actual} differs from {expected} (relative tolerance {tolerance})"
        )
        .into());
    }
    Ok(())
}
pub fn run(gpu: Option<&Gpu>) -> Result<Value> {
    let fixture: Value = serde_json::from_str(include_str!("../../tests/uff-reference.json"))?;
    let provenance: Value = serde_json::from_str(include_str!("../../data/uff/provenance.json"))?;
    if fixture["source_commit"] != provenance["commit"]
        || fixture["data_sha256"] != provenance["files"]["DEMO_GALAXY.csv"]
    {
        return Err("UFF reference provenance mismatch".into());
    }
    let radii = fixture["radii_kpc"].as_array().unwrap();
    let mut cases = Vec::new();
    let mut expected = Vec::new();
    for entry in fixture["cases"].as_array().unwrap() {
        let settings = &entry["settings"];
        let mut p = Physics::default();
        for (old, new) in [
            ("diskML", "disk_ml"),
            ("bulgeML", "bulge_ml"),
            ("blackHoleMillion", "black_hole_million"),
            ("uffVInf", "uff_v_inf"),
            ("uffCore", "uff_core"),
            ("uffBeta", "uff_beta"),
            ("haloLogMass", "halo_log_mass"),
            ("haloConcentration", "halo_concentration"),
            ("burkertLogDensity", "burkert_log_density"),
            ("burkertCore", "burkert_core"),
            ("mondA0", "mond_a0"),
        ] {
            if let Some(v) = settings.get(old) {
                let mut object = serde_json::to_value(&p)?;
                object[new] = v.clone();
                p = serde_json::from_value(object)?;
            }
        }
        for model in Model::ALL {
            p.model = model;
            p.validate()?;
            let predictions = entry["predictions"][model.name()].as_array().unwrap();
            for (i, v) in predictions.iter().enumerate() {
                let r = radii[i].as_f64().unwrap();
                let v = v.as_f64().unwrap();
                close(reference::velocity(r, &p), v, 2e-8, "native UFF reference")?;
                cases.push(Case::curve(r, &p));
                expected.push(v);
            }
        }
    }
    if let Some(gpu) = gpu {
        for (p, v) in gpu.evaluate("curves", &cases)?.iter().zip(&expected) {
            close(p.orbit[0] as f64, *v, 2e-4, "GPU UFF reference")?;
        }
    }
    let compact_fixture: Value =
        serde_json::from_str(include_str!("../tests/compact-reference.json"))?;
    if compact_fixture["source_commit"] != provenance["commit"]
        || compact_fixture["source_sha256"] != provenance["files"]["uff/compact.py"]
    {
        return Err("Compact reference provenance mismatch".into());
    }
    close(
        reference::area_gap(),
        compact_fixture["area_gap_m2"].as_f64().unwrap(),
        2e-12,
        "LQG area scale",
    )?;
    let mut compact_cases = Vec::new();
    let mut compact_expected = Vec::new();
    for row in compact_fixture["cases"].as_array().unwrap() {
        let mass = row["mass_msun"].as_f64().unwrap();
        let spin = row["spin"].as_f64().unwrap();
        let truth: Vec<f64> = row["radii"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        for (a, b) in reference::compact(mass, spin).into_iter().zip(&truth) {
            close(a, *b, 2e-12, "native Kerr reference")?;
        }
        compact_cases.push(Case::compact(mass, spin));
        compact_expected.push(truth);
    }
    if let Some(gpu) = gpu {
        for (result, truth) in gpu
            .evaluate("compact", &compact_cases)?
            .iter()
            .zip(compact_expected)
        {
            for (a, b) in result.orbit.into_iter().zip(truth) {
                close(a as f64, b, 8e-5, "GPU Kerr reference")?;
            }
        }
        for integrator in [Integrator::Circular, Integrator::Leapfrog] {
            for direction in [-1, 1] {
                let s = Spin {
                    particles: 1031,
                    snapshot_limit: 257,
                    steps: 80,
                    dt_myr: 0.05,
                    integrator,
                    direction,
                    radial_kick_kms: if integrator == Integrator::Leapfrog {
                        10.0
                    } else {
                        0.0
                    },
                    ..Spin::default()
                };
                let mut cpu = reference::initialize(&s);
                let field = gpu.initialize(&s)?;
                reference::advance(&mut cpu, &s, s.steps, s.steps);
                gpu.advance(&field, &s, s.steps, s.steps);
                let actual = gpu.sample(&field, &s)?;
                let expected = reference::sample(&cpu, s.sample_count());
                for (a, b) in actual.iter().zip(expected) {
                    for (x, y) in a.state.iter().zip(b.state) {
                        if !x.is_finite() || (*x - y).abs() > 3e-4 * (1.0 + y.abs()) {
                            return Err(format!("GPU orbit/reference mismatch: {x} vs {y}").into());
                        }
                    }
                }
            }
        }
        // i*particles exceeds u32 here; verify the GPU's exact gather mapping.
        let s = Spin {
            particles: 262147,
            snapshot_limit: 65536,
            ..Spin::default()
        };
        let field = gpu.initialize(&s)?;
        let actual = gpu.sample(&field, &s)?;
        let cpu = reference::initialize(&s);
        for i in [0, 1, 32768, 65534, 65535] {
            let expected = cpu[reference::sample_index(i, s.sample_count(), s.particles) as usize];
            for (x, y) in actual[i as usize].state.iter().zip(expected.state) {
                if (*x - y).abs() > 3e-4 * (1.0 + y.abs()) {
                    return Err("Large-field GPU gather index mismatch".into());
                }
            }
        }
    }
    Ok(
        json!({"status":"passed","uff_python_predictions":expected.len(),"compact_python_predictions":compact_cases.len(),
        "gpu_checked":gpu.is_some(),"adapter":gpu.map(|g|&g.info),"gpu_velocity_relative_tolerance":2e-4,
        "gpu_compact_relative_tolerance":8e-5,"orbit_comparison_cases":if gpu.is_some() {4} else {0},
        "large_gather_particles":if gpu.is_some() {262147} else {0}}),
    )
}
