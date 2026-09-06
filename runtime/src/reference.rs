// SPDX-License-Identifier: Apache-2.0
use crate::config::{Integrator, Physics, Spin};
use bytemuck::{Pod, Zeroable};
use galaxy_sampler::physics::{self, G, KPC_TO_M};
pub const KMS_TO_KPC_MYR: f64 = 31557600.0 * 1e6 * 1000.0 / KPC_TO_M;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Particle {
    pub orbit: [f32; 4],
    pub state: [f32; 4],
}
pub fn sample_index(i: u32, count: u32, total: u32) -> u32 {
    (i as u64 * total as u64 / count as u64) as u32
}
pub fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^ (x >> 16)
}
pub fn random(index: u32, seed: u32, lane: u32) -> f64 {
    (hash(index ^ seed ^ (lane + 1).wrapping_mul(0x9e3779b9)) >> 8) as f64 / 16777216.0
}
pub fn velocity(radius: f64, p: &Physics) -> f64 {
    physics::velocity_kms(radius, &p.native()).expect("validated physical parameters and radius")
}

pub fn initialize(s: &Spin) -> Vec<Particle> {
    (0..s.particles)
        .map(|i| {
            let u = random(i, s.seed, 0);
            let v = random(i, s.seed, 1);
            let w = random(i, s.seed, 2);
            let kind = random(i, s.seed, 4);
            let bulge = kind < s.bulge_fraction as f32 as f64;
            let halo = kind >= 0.97;
            let r = 12.0 * physics::star_radius(u, kind, s.bulge_fraction);
            let theta = if bulge || halo {
                std::f64::consts::TAU * v
            } else {
                (v * s.arms as f64).floor() * std::f64::consts::TAU / s.arms as f64
                    + (r / 1.2).ln() / s.pitch_deg.to_radians().tan()
                    + (w - 0.5) * s.scatter
            };
            let z = (random(i, s.seed, 3) - 0.5)
                * 2.0
                * if bulge {
                    2.28 * (1.0 - r / 3.6)
                } else if halo {
                    4.8
                } else {
                    s.thickness_kpc * (0.4 + r / 12.0)
                };
            let speed = if s.integrator == Integrator::Leapfrog {
                let soft_r = (r * r + s.softening_kpc.powi(2)).sqrt();
                velocity(soft_r, &s.physics) * r / soft_r
            } else {
                velocity(r, &s.physics)
            } * KMS_TO_KPC_MYR;
            let omega = s.direction as f64 * speed / r;
            let kick = s.radial_kick_kms * KMS_TO_KPC_MYR;
            Particle {
                orbit: [r as f32, theta as f32, z as f32, omega as f32],
                state: [
                    (r * theta.cos()) as f32,
                    (r * theta.sin()) as f32,
                    (-omega * r * theta.sin() + kick * theta.cos()) as f32,
                    (omega * r * theta.cos() + kick * theta.sin()) as f32,
                ],
            }
        })
        .collect()
}
pub fn acceleration(x: f64, y: f64, s: &Spin) -> [f64; 2] {
    let r = (x * x + y * y + s.softening_kpc.powi(2)).sqrt();
    let factor = -(velocity(r, &s.physics) * KMS_TO_KPC_MYR).powi(2) / (r * r);
    [factor * x, factor * y]
}
pub fn advance(particles: &mut [Particle], s: &Spin, steps: u32, total_steps: u32) {
    if s.integrator == Integrator::Circular {
        let time = total_steps as f64 * s.dt_myr;
        for p in particles {
            let r = p.orbit[0] as f64;
            let omega = p.orbit[3] as f64;
            let theta = p.orbit[1] as f64 + omega * time;
            p.state = [
                (r * theta.cos()) as f32,
                (r * theta.sin()) as f32,
                (-omega * r * theta.sin()) as f32,
                (omega * r * theta.cos()) as f32,
            ];
        }
    } else {
        for _ in 0..steps {
            for p in &mut *particles {
                let [x, y, vx, vy] = p.state.map(f64::from);
                let dt = s.dt_myr;
                let a = acceleration(x, y, s);
                let vx = vx + 0.5 * dt * a[0];
                let vy = vy + 0.5 * dt * a[1];
                let x = x + dt * vx;
                let y = y + dt * vy;
                let a = acceleration(x, y, s);
                p.state = [
                    x as f32,
                    y as f32,
                    (vx + 0.5 * dt * a[0]) as f32,
                    (vy + 0.5 * dt * a[1]) as f32,
                ];
            }
        }
    }
}
pub fn sample(particles: &[Particle], count: u32) -> Vec<Particle> {
    (0..count)
        .map(|i| particles[sample_index(i, count, particles.len() as u32) as usize])
        .collect()
}

// UFF compact.py: signed equatorial Kerr radii, in gravitational-radius units.
// Tiny LQG scale ratios deliberately stay in f64 on the CPU.
pub fn compact(mass: f64, spin: f64) -> [f64; 4] {
    let rg = 6.67430e-11 * mass * 1.98847e30 / 299792458.0_f64.powi(2) / KPC_TO_M;
    let z1 = 1.0 + (1.0 - spin * spin).cbrt() * ((1.0 + spin).cbrt() + (1.0 - spin).cbrt());
    let z2 = (3.0 * spin * spin + z1 * z1).sqrt();
    let sign = if spin == 0.0 { 0.0 } else { spin.signum() };
    [
        1.0 + (1.0 - spin * spin).sqrt(),
        2.0 * (1.0 + ((2.0 / 3.0) * (-spin).acos()).cos()),
        3.0 + z2 - sign * ((3.0 - z1) * (3.0 + z1 + 2.0 * z2)).max(0.0).sqrt(),
        rg,
    ]
}
pub fn influence_radius(mass: f64, sigma: f64) -> f64 {
    G * mass / (sigma * sigma)
}
pub fn area_gap() -> f64 {
    4.0 * 3.0_f64.sqrt() * std::f64::consts::PI * 0.2375 * (1.616255e-35_f64).powi(2)
}
