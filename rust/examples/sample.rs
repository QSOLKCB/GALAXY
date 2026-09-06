// SPDX-License-Identifier: Apache-2.0
//! Local native sampler exercise; measures generation, not graphics frame rate.
use galaxy_sampler::{logical_index, make_sample};
use std::time::Instant;

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() > 3 || args.iter().any(|arg| arg == "--help") {
        return Err("Usage: sample [logical exponent 1..32] [rendered count 1..65536] [u32 seed]".into());
    }
    let exponent: u32 = args.first().map_or("32", String::as_str).parse().map_err(|_| "Invalid exponent")?;
    let count: usize = args.get(1).map_or("65536", String::as_str).parse().map_err(|_| "Invalid rendered count")?;
    let seed: u32 = args.get(2).map_or("303", String::as_str).parse().map_err(|_| "Invalid seed")?;
    if !(1..=32).contains(&exponent) { return Err("Logical exponent must be between 1 and 32".into()); }
    let logical = 1_u64 << exponent;
    let start = Instant::now();
    let sample = make_sample(logical, count, seed).ok_or("Rendered count must be 1..65536 and no greater than the logical population")?;
    let elapsed = start.elapsed();
    println!("Logical stars: {logical} (2^{exponent})");
    println!("Sampled stars: {count}");
    println!("Particle buffer: {} bytes", sample.len() * size_of::<f32>());
    println!("Last logical ID: {}", logical_index(count - 1, count, logical));
    println!("Generation: {:.3} ms (native sampler only; no rendering)", elapsed.as_secs_f64() * 1000.0);
    println!("Seed: {seed}; first star's 8 parameters: {:?}", &sample[..8]);
    Ok(())
}

fn main() {
    if let Err(message) = run() { eprintln!("{message}"); std::process::exit(2); }
}
