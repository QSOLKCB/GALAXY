// SPDX-License-Identifier: Apache-2.0
//! A logical population is indexed, never allocated in full.
//! The browser uploads this bounded float32 buffer to WebGL once per reseed.
use std::cell::RefCell;
pub mod physics;
mod uff_data;

pub const MAX_LOGICAL: u64 = 1_u64 << 32;
pub const MAX_RENDERED: usize = 65_536;
pub const STRIDE: usize = 8;

thread_local! {
    // One worker/thread owns its buffer; exported pointers remain valid until
    // the next generate() call. JS reacquires memory.buffer after every call.
    static SAMPLE: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
    static ORBIT_RATES: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

pub fn logical_index(index: usize, count: usize, logical: u64) -> u32 {
    (index as u64 * logical / count as u64) as u32
}

pub fn hash32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846ca68b);
    value ^ (value >> 16)
}

pub fn sample_value(id: u32, seed: u32, lane: usize) -> f32 {
    // Twenty-four random bits are exactly representable in both f32 and JS.
    (hash32(id ^ seed ^ (lane as u32 + 1).wrapping_mul(0x9e3779b9)) >> 8) as f32
        / 16_777_216.0
}

pub fn make_sample(logical: u64, count: usize, seed: u32) -> Option<Vec<f32>> {
    if !(1..=MAX_LOGICAL).contains(&logical)
        || !(1..=MAX_RENDERED).contains(&count)
        || count as u64 > logical
    {
        return None;
    }
    let mut data = vec![0.0; count * STRIDE];
    for (index, star) in data.chunks_exact_mut(STRIDE).enumerate() {
        let id = logical_index(index, count, logical);
        for (lane, value) in star.iter_mut().enumerate() {
            *value = sample_value(id, seed, lane);
        }
    }
    Some(data)
}

#[no_mangle]
pub extern "C" fn abi_version() -> u32 { 2 }

#[no_mangle]
pub extern "C" fn max_rendered() -> u32 { MAX_RENDERED as u32 }

// f64 accepts the exact Number 2^32 without JS BigInt or u32 truncation.
#[no_mangle]
pub extern "C" fn generate(logical: f64, count: u32, seed: u32) -> u32 {
    ORBIT_RATES.with(|rates| rates.borrow_mut().clear());
    let valid = logical.is_finite()
        && logical >= 1.0
        && logical <= MAX_LOGICAL as f64
        && logical.fract() == 0.0;
    let data = if valid { make_sample(logical as u64, count as usize, seed) } else { None };
    SAMPLE.with(|sample| {
        let mut buffer = sample.borrow_mut();
        *buffer = data.unwrap_or_default();
        (buffer.len() / STRIDE) as u32
    })
}

#[no_mangle]
pub extern "C" fn buffer_ptr() -> *const f32 {
    SAMPLE.with(|sample| sample.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn buffer_len() -> u32 {
    SAMPLE.with(|sample| sample.borrow().len() as u32)
}

/// Recompute one angular rate per existing sampled star; no full population allocation.
#[no_mangle]
pub extern "C" fn configure_dynamics(
    model: u32, disk_ml: f64, bulge_ml: f64, black_hole_million: f64,
    uff_v_inf: f64, uff_core: f64, uff_beta: f64, halo_log_mass: f64,
    halo_concentration: f64, burkert_log_density: f64, burkert_core: f64,
    mond_a0: f64, bulge: f64, shear: f64,
) -> u32 {
    let p = physics::Parameters { model, disk_ml, bulge_ml, black_hole_million, uff_v_inf,
        uff_core, uff_beta, halo_log_mass, halo_concentration, burkert_log_density, burkert_core, mond_a0 };
    ORBIT_RATES.with(|rates| {
        let mut buffer = rates.borrow_mut();
        buffer.clear();
        if !p.valid() || !bulge.is_finite() || !(0.0..=0.5).contains(&bulge) ||
            !shear.is_finite() || !(0.0..=1.0).contains(&shear) { return 0; }
        SAMPLE.with(|sample| {
            for star in sample.borrow().chunks_exact(STRIDE) {
                let radius = physics::star_radius(star[0] as f64, star[4] as f64, bulge);
                let Some(rate) = physics::angular_rate(radius, &p, shear) else { buffer.clear(); return 0; };
                buffer.push(rate as f32);
            }
            buffer.len() as u32
        })
    })
}

#[no_mangle]
pub extern "C" fn orbit_ptr() -> *const f32 { ORBIT_RATES.with(|rates| rates.borrow().as_ptr()) }
#[no_mangle]
pub extern "C" fn orbit_len() -> u32 { ORBIT_RATES.with(|rates| rates.borrow().len() as u32) }

/// Scalar diagnostic for parity checks and direct Wasm users. Invalid requests return NaN.
#[no_mangle]
pub extern "C" fn circular_velocity(
    radius: f64, model: u32, disk_ml: f64, bulge_ml: f64, black_hole_million: f64,
    uff_v_inf: f64, uff_core: f64, uff_beta: f64, halo_log_mass: f64,
    halo_concentration: f64, burkert_log_density: f64, burkert_core: f64, mond_a0: f64,
) -> f64 {
    let p = physics::Parameters { model, disk_ml, bulge_ml, black_hole_million, uff_v_inf,
        uff_core, uff_beta, halo_log_mass, halo_concentration, burkert_log_density, burkert_core, mond_a0 };
    physics::velocity_kms(radius, &p).unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximum_population_is_sampled_without_wrapping() {
        let last = logical_index(MAX_RENDERED - 1, MAX_RENDERED, MAX_LOGICAL);
        assert_eq!(last, 4_294_901_760);
        let data = make_sample(MAX_LOGICAL, MAX_RENDERED, 303).unwrap();
        assert_eq!(data.len() * size_of::<f32>(), 2 * 1024 * 1024);
        assert!(data.iter().all(|x| x.is_finite() && *x >= 0.0 && *x < 1.0));
        assert_eq!(data, make_sample(MAX_LOGICAL, MAX_RENDERED, 303).unwrap());
        assert_ne!(data, make_sample(MAX_LOGICAL, MAX_RENDERED, 304).unwrap());
    }

    #[test]
    fn samples_span_population_even_for_non_power_of_two_counts() {
        let ids: Vec<_> = (0..999).map(|i| logical_index(i, 999, MAX_LOGICAL)).collect();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(ids[998] > 4_290_000_000);
    }

    #[test]
    fn rejected_generation_clears_previous_buffer() {
        assert_eq!(generate(65536.0, 512, 303), 512);
        for logical in [f64::NAN, f64::INFINITY, -1.0, 0.5, MAX_LOGICAL as f64 + 1.0] {
            assert_eq!(generate(logical, 512, 303), 0);
            assert_eq!(buffer_len(), 0);
        }
        assert_eq!(generate(65536.0, 65537, 303), 0);
        assert_eq!(generate(1.0, 512, 303), 0);
        assert_eq!(generate(65536.0, 0, 303), 0);
    }
}
