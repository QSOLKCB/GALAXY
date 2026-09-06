// SPDX-License-Identifier: Apache-2.0
// Adapted from QSOLKCB/UFF, commit 596cd732df61587aa1a9801cad1ec13483b1347f.
// Copyright 2025-2026 Trent Slade / QSOL-IMC. See data/uff/NOTICE.
use crate::uff_data::DEMO;
use std::f64::consts::PI;

pub const KPC_TO_M: f64 = 3.085677581491367e19;
pub const G: f64 = 6.67430e-11 * 1.98847e30 / (KPC_TO_M * 1e6);
pub const DISK_KPC: f64 = 12.0;
pub const MYR_PER_CLOCK: f64 = 20.0;
pub const RATE_CONVERSION: f64 = 31557600.0 * 1e6 * 1000.0 / KPC_TO_M * MYR_PER_CLOCK;

#[derive(Clone, Copy, Debug)]
pub struct Parameters {
    /// 0: original visual law, 1: baryons, 2: NFW, 3: Burkert, 4: MOND/RAR, 5: UFF.
    pub model: u32,
    pub disk_ml: f64,
    pub bulge_ml: f64,
    pub black_hole_million: f64,
    pub uff_v_inf: f64,
    pub uff_core: f64,
    pub uff_beta: f64,
    pub halo_log_mass: f64,
    pub halo_concentration: f64,
    pub burkert_log_density: f64,
    pub burkert_core: f64,
    pub mond_a0: f64,
}

impl Default for Parameters {
    fn default() -> Self {
        Self { model: 5, disk_ml: 0.5, bulge_ml: 0.7, black_hole_million: 0.0,
            uff_v_inf: 120.0, uff_core: 3.0, uff_beta: 0.0, halo_log_mass: 11.5,
            halo_concentration: 10.0, burkert_log_density: 7.5, burkert_core: 5.0, mond_a0: 1.2 }
    }
}
impl Parameters {
    pub fn valid(&self) -> bool {
        self.model <= 5 && [
            (self.disk_ml, 0.0, 1.5), (self.bulge_ml, 0.0, 2.0),
            (self.black_hole_million, 0.0, 1000.0), (self.uff_v_inf, 0.0, 500.0),
            (self.uff_core, 0.02, 100.0), (self.uff_beta, -1.0, 1.0),
            (self.halo_log_mass, 8.0, 14.5), (self.halo_concentration, 1.0, 40.0),
            (self.burkert_log_density, 4.0, 11.0), (self.burkert_core, 0.05, 100.0),
            (self.mond_a0, 0.03, 6.3),
        ].iter().all(|(value, min, max)| value.is_finite() && value >= min && value <= max)
    }
}

pub fn components_at(radius: f64) -> [f64; 3] {
    // Matches UFF/NumPy's linear interpolation and constant endpoint extension.
    let mut a = &DEMO[0];
    if radius <= a[0] { return [a[3], a[4], a[5]]; }
    for b in DEMO.iter().skip(1) {
        if radius <= b[0] {
            let t = (radius - a[0]) / (b[0] - a[0]);
            return [3, 4, 5].map(|column| a[column] + t * (b[column] - a[column]));
        }
        a = b;
    }
    [a[3], a[4], a[5]]
}
pub fn baryonic_v2(gas: f64, disk: f64, bulge: f64, disk_ml: f64, bulge_ml: f64) -> f64 {
    (gas * gas.abs() + disk_ml * disk * disk + bulge_ml * bulge * bulge).max(0.0)
}
pub fn nfw_shape(x: f64) -> f64 {
    if x < 1e-4 { 0.5 * x.powi(2) - 2.0 / 3.0 * x.powi(3) + 0.75 * x.powi(4) }
    else { x.ln_1p() - x / (1.0 + x) }
}
pub fn nfw_r200(mass: f64) -> f64 {
    let rho_critical = 3.0 * 0.07_f64.powi(2) / (8.0 * PI * G);
    (3.0 * mass / (4.0 * PI * 200.0 * rho_critical)).cbrt()
}
fn halo_v2(radius: f64, p: &Parameters) -> f64 {
    match p.model {
        2 => {
            let mass = 10.0_f64.powf(p.halo_log_mass);
            G * mass * nfw_shape(p.halo_concentration * radius / nfw_r200(mass))
                / nfw_shape(p.halo_concentration) / radius
        }
        3 => {
            let x = radius / p.burkert_core;
            let shape = if x < 1e-3 { 4.0 / 3.0 * x.powi(3) }
                else { ((1.0 + x).powi(2) * (1.0 + x * x)).ln() - 2.0 * x.atan() };
            G * PI * 10.0_f64.powf(p.burkert_log_density) * p.burkert_core.powi(3) * shape / radius
        }
        5 => {
            let x = radius / p.uff_core;
            let base = if x < 1e-3 { x * x * (1.0 / 3.0 - x * x / 5.0 + x.powi(4) / 7.0) }
                else { (1.0 - x.atan() / x).max(0.0) };
            p.uff_v_inf.powi(2) * base * (2.0 * p.uff_beta * x / (1.0 + x)).exp()
        }
        _ => 0.0,
    }
}
pub fn velocity_kms(radius: f64, p: &Parameters) -> Option<f64> {
    if !radius.is_finite() || radius <= 0.0 || !p.valid() || p.model == 0 { return None; }
    let [gas, disk, bulge] = components_at(radius);
    let mut total = baryonic_v2(gas, disk, bulge, p.disk_ml, p.bulge_ml)
        + G * p.black_hole_million * 1e6 / radius + halo_v2(radius, p);
    if p.model == 4 && total > 0.0 {
        let y = total * 1e6 / (radius * KPC_TO_M) / (p.mond_a0 * 1e-10);
        total /= -(-y.sqrt()).exp_m1();
    }
    let value = total.sqrt();
    if value.is_finite() { Some(value) } else { None }
}
pub fn star_radius(u: f64, kind: f64, bulge: f64) -> f64 {
    if kind < (bulge as f32) as f64 { 0.015 + 0.27 * u.powf(1.8) }
    else if kind >= 0.97 { 0.25 + 0.85 * u }
    else { 0.06 + 0.92 * u.powf(1.4) }
}
pub fn angular_rate(radius: f64, p: &Parameters, shear: f64) -> Option<f64> {
    if !radius.is_finite() || radius <= 0.0 || !p.valid() || !shear.is_finite() || !(0.0..=1.0).contains(&shear) { return None; }
    if p.model == 0 { return Some(0.32 * ((1.0 - shear) + shear / (radius * radius + 0.0144).sqrt())); }
    let physical_radius = radius * DISK_KPC;
    velocity_kms(physical_radius, p).map(|v| v / physical_radius * RATE_CONVERSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_gas_and_stellar_mass_scale_velocity_squared() {
        assert_eq!(baryonic_v2(-10.0, 20.0, 10.0, 0.5, 0.7), 170.0);
        assert_eq!(baryonic_v2(-100.0, 20.0, 10.0, 0.5, 0.7), 0.0);
        assert_eq!(components_at(0.1), [5.0, 20.0, 10.0]);
        assert_eq!(components_at(30.0), [15.0, 95.0, 5.0]);
    }
    #[test]
    fn physical_model_limits_and_invalid_inputs() {
        let mut p = Parameters::default();
        p.model = 1;
        let baryons = velocity_kms(8.0, &p).unwrap();
        p.model = 5; p.uff_v_inf = 0.0;
        assert_eq!(velocity_kms(8.0, &p).unwrap(), baryons);
        p.model = 4;
        assert!(velocity_kms(8.0, &p).unwrap() > baryons);
        p.model = 1; p.black_hole_million = 100.0;
        let difference = velocity_kms(8.0, &p).unwrap().powi(2) - baryons.powi(2);
        assert!((difference - G * 1e8 / 8.0).abs() < 1e-9);
        assert!(velocity_kms(0.0, &p).is_none());
        p.uff_core = f64::NAN;
        assert!(velocity_kms(8.0, &p).is_none());
    }
    #[test]
    fn circular_orbit_units_and_full_sample_are_finite() {
        let mut p = Parameters::default();
        for model in 0..=5 {
            p.model = model;
            for i in 0..65536 {
                let r = 0.015 + 1.085 * i as f64 / 65535.0;
                assert!(angular_rate(r, &p, 0.35).unwrap() > 0.0);
            }
        }
        p.model = 5;
        let r = 0.5;
        let v = velocity_kms(r * DISK_KPC, &p).unwrap();
        assert!((angular_rate(r, &p, 0.0).unwrap() - v / (r * DISK_KPC) * RATE_CONVERSION).abs() < 1e-12);
    }
}
