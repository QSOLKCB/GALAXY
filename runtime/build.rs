// SPDX-License-Identifier: Apache-2.0
use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};
fn main() {
    let files = [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "src/main.rs",
        "src/lib.rs",
        "src/config.rs",
        "src/reference.rs",
        "src/gpu.rs",
        "src/kernels.wgsl",
        "src/output.rs",
        "src/verify.rs",
        "../rust/src/lib.rs",
        "../rust/src/physics.rs",
        "../rust/src/uff_data.rs",
        "../data/uff/DEMO_GALAXY.csv",
        "../data/uff/provenance.json",
        "../rust/Cargo.toml",
        "../rust-toolchain.toml",
        "../tests/uff-reference.json",
        "tests/compact-reference.json",
    ];
    let mut hash = Sha256::new();
    for file in files {
        println!("cargo:rerun-if-changed={file}");
        let content = fs::read(file).expect(file);
        hash.update(file.as_bytes());
        hash.update([0]);
        hash.update((content.len() as u64).to_le_bytes());
        hash.update(content);
    }
    println!(
        "cargo:rustc-env=GALAXY_RUNTIME_SOURCE_SHA256={:x}",
        hash.finalize()
    );
    let csv = fs::read_to_string("../data/uff/DEMO_GALAXY.csv").unwrap();
    let rows: Vec<Vec<f64>> = csv
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|line| {
            line.split(',')
                .take(6)
                .map(|x| x.parse().unwrap())
                .collect()
        })
        .collect();
    assert!(rows.len() >= 2 && rows.iter().all(|r| r.len() == 6));
    let radii = rows
        .iter()
        .map(|r| format!("{:?}", r[0]))
        .collect::<Vec<_>>()
        .join(",");
    let components = rows
        .iter()
        .map(|r| format!("vec3<f32>({:?},{:?},{:?})", r[3], r[4], r[5]))
        .collect::<Vec<_>>()
        .join(",");
    let source = format!("const G: f32 = {:?};\nconst KMS_TO_KPC_MYR: f32 = {:?};\nconst DEMO_N: u32 = {}u;\nconst DEMO_R = array<f32, {}>({});\nconst DEMO_V = array<vec3<f32>, {}>({});\n",
        6.67430e-11 * 1.98847e30 / (3.085677581491367e19 * 1e6),
        31557600.0 * 1e6 * 1000.0 / 3.085677581491367e19, rows.len(), rows.len(), radii, rows.len(), components);
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("uff-data.wgsl"),
        source,
    )
    .unwrap();
}
