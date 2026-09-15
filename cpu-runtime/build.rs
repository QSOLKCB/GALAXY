// SPDX-License-Identifier: Apache-2.0
use std::{env, fs, path::PathBuf};

fn sanitize_inner_docs(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            if let Some(rest) = line.strip_prefix("//!") {
                format!("//{rest}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn emit(source: &str, output: &str) {
    let contents = fs::read_to_string(source)
        .unwrap_or_else(|error| panic!("cannot read {source}: {error}"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join(output), sanitize_inner_docs(&contents))
        .unwrap_or_else(|error| panic!("cannot write generated {output}: {error}"));
    println!("cargo:rerun-if-changed={source}");
}

fn main() {
    emit("src/main.rs", "legacy_runtime.inc.rs");
    emit("src/bin/worker_soa_probe.rs", "worker_soa_probe.inc.rs");
    emit("src/bin/persistent_soa.rs", "persistent_soa.inc.rs");
}
