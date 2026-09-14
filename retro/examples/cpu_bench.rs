// SPDX-License-Identifier: Apache-2.0
use galaxy_retro_math::{hash32, sin_cos_q30};
use std::{env, hint::black_box, time::Instant};

const TURN_SCALE: f64 = std::f64::consts::TAU / 4_294_967_296.0;
const LUT_BITS: u32 = 14;
const LUT_SIZE: usize = 1 << LUT_BITS;
const LUT_SHIFT: u32 = 32 - LUT_BITS;
const LUT_FRAC_MASK: u32 = (1_u32 << LUT_SHIFT) - 1;

struct Lut {
    cos: Vec<i32>,
    sin: Vec<i32>,
}

impl Lut {
    fn build() -> Self {
        let mut cos = Vec::with_capacity(LUT_SIZE);
        let mut sin = Vec::with_capacity(LUT_SIZE);
        for index in 0..LUT_SIZE {
            let angle = (index as u32) << LUT_SHIFT;
            let (c, s) = sin_cos_q30(angle);
            cos.push(c as i32);
            sin.push(s as i32);
        }
        Self { cos, sin }
    }

    fn sin_cos(&self, angle: u32) -> (i64, i64) {
        let index = (angle >> LUT_SHIFT) as usize;
        let next = (index + 1) & (LUT_SIZE - 1);
        let fraction = (angle & LUT_FRAC_MASK) as i64;
        let c0 = self.cos[index] as i64;
        let s0 = self.sin[index] as i64;
        let c = c0 + (((self.cos[next] as i64 - c0) * fraction) >> LUT_SHIFT);
        let s = s0 + (((self.sin[next] as i64 - s0) * fraction) >> LUT_SHIFT);
        (c, s)
    }
}

fn positive_env(name: &str, fallback: usize, maximum: usize) -> usize {
    match env::var(name) {
        Ok(raw) => {
            let value = raw
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("{name} must be a positive integer"));
            assert!(
                (1..=maximum).contains(&value),
                "{name} must be in 1..={maximum}"
            );
            value
        }
        Err(_) => fallback,
    }
}

fn radius_q16(index: usize) -> i32 {
    let lane = (hash32(index as u32 ^ 0xa511_e9b3) & 0xffff) as i64;
    (16_384 + lane * 49_152 / 65_535) as i32
}

fn inputs(samples: usize) -> (Vec<u32>, Vec<i32>) {
    let mut angles = Vec::with_capacity(samples);
    let mut radii = Vec::with_capacity(samples);
    let mut angle = 0x1234_5678_u32;
    for index in 0..samples {
        angle = angle.wrapping_add(0x9e37_79b9_u32 ^ hash32(index as u32));
        angles.push(angle);
        radii.push(radius_q16(index));
    }
    (angles, radii)
}

fn float_projection(angles: &[u32], radii: &[i32]) -> u64 {
    let mut checksum = 0.0_f64;
    for (&angle, &radius_q16) in angles.iter().zip(radii) {
        let radians = angle as f64 * TURN_SCALE;
        let (sin, cos) = radians.sin_cos();
        let radius = radius_q16 as f64;
        let x = radius * cos;
        let y = radius * sin;
        checksum += x - y;
    }
    checksum.to_bits()
}

fn cordic_projection(angles: &[u32], radii: &[i32]) -> u64 {
    let mut checksum = 0_i64;
    for (&angle, &radius_q16) in angles.iter().zip(radii) {
        let (cos, sin) = sin_cos_q30(angle);
        let radius = radius_q16 as i64;
        let x = (radius * cos) >> 30;
        let y = (radius * sin) >> 30;
        checksum = checksum.wrapping_add(x).wrapping_sub(y);
    }
    checksum as u64
}

fn lut_projection(lut: &Lut, angles: &[u32], radii: &[i32]) -> u64 {
    let mut checksum = 0_i64;
    for (&angle, &radius_q16) in angles.iter().zip(radii) {
        let (cos, sin) = lut.sin_cos(angle);
        let radius = radius_q16 as i64;
        let x = (radius * cos) >> 30;
        let y = (radius * sin) >> 30;
        checksum = checksum.wrapping_add(x).wrapping_sub(y);
    }
    checksum as u64
}

fn median_sorted(timings: &[u128]) -> u128 {
    let middle = timings.len() / 2;
    if timings.len() % 2 == 0 {
        let lower = timings[middle - 1];
        let upper = timings[middle];
        lower + (upper - lower) / 2
    } else {
        timings[middle]
    }
}

fn measure<F>(repeats: usize, mut function: F) -> (u128, u128, u64)
where
    F: FnMut() -> u64,
{
    let mut timings = Vec::with_capacity(repeats);
    let mut checksum = None;
    for _ in 0..repeats {
        let started = Instant::now();
        let result = function();
        let elapsed = started.elapsed().as_nanos();

        // Keep the result observable outside the timed region without placing a
        // compiler barrier inside the per-sample hot loop. Each repeat must
        // produce the same deterministic checksum.
        let result = black_box(result);
        if let Some(expected) = checksum {
            assert_eq!(result, expected, "benchmark checksum changed between repeats");
        } else {
            checksum = Some(result);
        }
        timings.push(elapsed);
    }
    timings.sort_unstable();
    let best = timings[0];
    let median = median_sorted(&timings);
    (best, median, checksum.expect("positive repeat count"))
}

fn report(method: &str, samples: usize, result: (u128, u128, u64)) {
    let (best, median, checksum) = result;
    let ns_per_sample = best as f64 / samples as f64;
    let per_second = if best == 0 {
        f64::INFINITY
    } else {
        samples as f64 * 1e9 / best as f64
    };
    println!("{method},{best},{median},{ns_per_sample:.6},{per_second:.3},{checksum:016x}");
}

fn lut_error(lut: &Lut) -> i64 {
    let mut maximum = 0_i64;
    for index in 0..8192_u32 {
        let angle = hash32(index.wrapping_mul(0x9e37_79b9));
        let (reference_cos, reference_sin) = sin_cos_q30(angle);
        let (lut_cos, lut_sin) = lut.sin_cos(angle);
        maximum = maximum.max((reference_cos - lut_cos).abs());
        maximum = maximum.max((reference_sin - lut_sin).abs());
    }
    maximum
}

fn main() {
    let samples = positive_env("GALAXY_CPU_BENCH_SAMPLES", 1_048_576, 16_777_216);
    let repeats = positive_env("GALAXY_CPU_BENCH_REPEATS", 5, 25);
    let (angles, radii) = inputs(samples);
    let lut = Lut::build();

    // Warm each path before recording timings so one-time page/cache effects are
    // not confused with the arithmetic comparison.
    black_box(float_projection(
        &angles[..angles.len().min(4096)],
        &radii[..radii.len().min(4096)],
    ));
    black_box(cordic_projection(
        &angles[..angles.len().min(4096)],
        &radii[..radii.len().min(4096)],
    ));
    black_box(lut_projection(
        &lut,
        &angles[..angles.len().min(4096)],
        &radii[..radii.len().min(4096)],
    ));

    println!("galaxy_retro_cpu_bench=v1");
    println!("arch={}", env::consts::ARCH);
    println!("os={}", env::consts::OS);
    println!("samples={samples}");
    println!("repeats={repeats}");
    println!("lut_entries={LUT_SIZE}");
    println!("lut_max_abs_q30_error={}", lut_error(&lut));
    println!("method,best_ns,median_ns,best_ns_per_sample,best_samples_per_second,checksum");

    let float = measure(repeats, || float_projection(&angles, &radii));
    let cordic = measure(repeats, || cordic_projection(&angles, &radii));
    let table = measure(repeats, || lut_projection(&lut, &angles, &radii));
    report("float_libm", samples, float);
    report("bam_cordic_q30", samples, cordic);
    report("bam_lut_q30", samples, table);

    println!(
        "float_over_cordic_best_ratio={:.6}",
        float.0 as f64 / cordic.0.max(1) as f64
    );
    println!(
        "float_over_lut_best_ratio={:.6}",
        float.0 as f64 / table.0.max(1) as f64
    );
    println!("note=ratios above 1 mean the retro method was faster for this projection microbenchmark");
}
