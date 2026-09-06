// SPDX-License-Identifier: Apache-2.0
use galaxy_sampler::physics::Parameters;
use serde::{Deserialize, Serialize};

pub const MAX_PARTICLES: u32 = 8_388_608;
pub const MAX_CASES: usize = 1_048_576;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Model {
    Baryons,
    Nfw,
    Burkert,
    MondRar,
    UffEmpirical,
}
impl Model {
    pub const ALL: [Self; 5] = [
        Self::Baryons,
        Self::Nfw,
        Self::Burkert,
        Self::MondRar,
        Self::UffEmpirical,
    ];
    pub fn id(self) -> u32 {
        match self {
            Self::Baryons => 1,
            Self::Nfw => 2,
            Self::Burkert => 3,
            Self::MondRar => 4,
            Self::UffEmpirical => 5,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Baryons => "baryons",
            Self::Nfw => "nfw",
            Self::Burkert => "burkert",
            Self::MondRar => "mond-rar",
            Self::UffEmpirical => "uff-empirical",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Physics {
    pub model: Model,
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
impl Default for Physics {
    fn default() -> Self {
        let p = Parameters::default();
        Self {
            model: Model::UffEmpirical,
            disk_ml: p.disk_ml,
            bulge_ml: p.bulge_ml,
            black_hole_million: p.black_hole_million,
            uff_v_inf: p.uff_v_inf,
            uff_core: p.uff_core,
            uff_beta: p.uff_beta,
            halo_log_mass: p.halo_log_mass,
            halo_concentration: p.halo_concentration,
            burkert_log_density: p.burkert_log_density,
            burkert_core: p.burkert_core,
            mond_a0: p.mond_a0,
        }
    }
}
impl Physics {
    pub fn native(&self) -> Parameters {
        Parameters {
            model: self.model.id(),
            disk_ml: self.disk_ml,
            bulge_ml: self.bulge_ml,
            black_hole_million: self.black_hole_million,
            uff_v_inf: self.uff_v_inf,
            uff_core: self.uff_core,
            uff_beta: self.uff_beta,
            halo_log_mass: self.halo_log_mass,
            halo_concentration: self.halo_concentration,
            burkert_log_density: self.burkert_log_density,
            burkert_core: self.burkert_core,
            mond_a0: self.mond_a0,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if !self.native().valid() {
            return Err(
                "Physics parameters are outside the UFF runtime bounds; see docs/GPU-RUNTIME.md"
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Integrator {
    Circular,
    Leapfrog,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Spin {
    pub physics: Physics,
    pub integrator: Integrator,
    pub particles: u32,
    pub seed: u32,
    pub steps: u32,
    pub dt_myr: f64,
    pub snapshot_every: u32,
    pub snapshot_limit: u32,
    pub arms: u32,
    pub pitch_deg: f64,
    pub scatter: f64,
    pub bulge_fraction: f64,
    pub thickness_kpc: f64,
    pub direction: i32,
    pub radial_kick_kms: f64,
    pub softening_kpc: f64,
    pub image_size: u32,
    pub extent_kpc: f64,
    pub inclination_deg: f64,
}
impl Default for Spin {
    fn default() -> Self {
        Self {
            physics: Physics::default(),
            integrator: Integrator::Circular,
            particles: 262_144,
            seed: 303,
            steps: 400,
            dt_myr: 0.25,
            snapshot_every: 100,
            snapshot_limit: 65_536,
            arms: 2,
            pitch_deg: 22.0,
            scatter: 0.45,
            bulge_fraction: 0.18,
            thickness_kpc: 0.3,
            direction: 1,
            radial_kick_kms: 0.0,
            softening_kpc: 0.02,
            image_size: 1024,
            extent_kpc: 16.0,
            inclination_deg: 38.0,
        }
    }
}
pub fn bounded(value: f64, min: f64, max: f64, name: &str) -> Result<()> {
    if !value.is_finite() || value < min || value > max {
        return Err(format!("{name} must be in [{min}, {max}]").into());
    }
    Ok(())
}
impl Spin {
    pub fn validate(&self) -> Result<()> {
        self.physics.validate()?;
        if self.particles == 0 || self.particles > MAX_PARTICLES {
            return Err(format!("particles must be in 1..={MAX_PARTICLES}").into());
        }
        if self.steps == 0 || self.steps > 100_000 {
            return Err("steps must be in 1..=100000".into());
        }
        if self.snapshot_limit == 0 || self.snapshot_limit > 65_536 {
            return Err("snapshot_limit must be in 1..=65536".into());
        }
        let snapshot_count = if self.snapshot_every == 0 {
            2
        } else {
            1 + self.steps.div_ceil(self.snapshot_every)
        };
        if snapshot_count > 256 {
            return Err("At most 256 snapshots per job; increase snapshot_every".into());
        }
        if !(1..=8).contains(&self.arms) || ![1, -1].contains(&self.direction) {
            return Err("arms must be 1..=8 and direction must be 1 or -1".into());
        }
        if !(128..=2048).contains(&self.image_size) {
            return Err("image_size must be in 128..=2048".into());
        }
        bounded(self.dt_myr, 0.0001, 2.0, "dt_myr")?;
        bounded(self.pitch_deg, 10.0, 40.0, "pitch_deg")?;
        bounded(self.scatter, 0.0, 2.5, "scatter")?;
        bounded(self.bulge_fraction, 0.0, 0.5, "bulge_fraction")?;
        bounded(self.thickness_kpc, 0.0, 2.0, "thickness_kpc")?;
        bounded(self.radial_kick_kms, -100.0, 100.0, "radial_kick_kms")?;
        bounded(self.softening_kpc, 0.001, 0.2, "softening_kpc")?;
        bounded(self.extent_kpc, 1.0, 100.0, "extent_kpc")?;
        bounded(self.inclination_deg, 0.0, 90.0, "inclination_deg")?;
        if self.integrator == Integrator::Circular && self.radial_kick_kms != 0.0 {
            return Err("radial_kick_kms requires the leapfrog integrator".into());
        }
        Ok(())
    }
    pub fn sample_count(&self) -> u32 {
        self.particles.min(self.snapshot_limit)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepParameter {
    DiskMl,
    BulgeMl,
    BlackHoleMillion,
    UffVInf,
    UffCore,
    UffBeta,
    HaloLogMass,
    HaloConcentration,
    BurkertLogDensity,
    BurkertCore,
    MondA0,
}
impl SweepParameter {
    pub fn set(self, p: &mut Physics, value: f64) {
        match self {
            Self::DiskMl => p.disk_ml = value,
            Self::BulgeMl => p.bulge_ml = value,
            Self::BlackHoleMillion => p.black_hole_million = value,
            Self::UffVInf => p.uff_v_inf = value,
            Self::UffCore => p.uff_core = value,
            Self::UffBeta => p.uff_beta = value,
            Self::HaloLogMass => p.halo_log_mass = value,
            Self::HaloConcentration => p.halo_concentration = value,
            Self::BurkertLogDensity => p.burkert_log_density = value,
            Self::BurkertCore => p.burkert_core = value,
            Self::MondA0 => p.mond_a0 = value,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sweep {
    pub parameter: SweepParameter,
    pub min: f64,
    pub max: f64,
    pub count: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Curves {
    pub physics: Physics,
    pub models: Vec<Model>,
    pub radius_min_kpc: f64,
    pub radius_max_kpc: f64,
    pub radii: u32,
    pub sweep: Option<Sweep>,
}
impl Default for Curves {
    fn default() -> Self {
        Self {
            physics: Physics::default(),
            models: Model::ALL.to_vec(),
            radius_min_kpc: 0.18,
            radius_max_kpc: 30.0,
            radii: 1024,
            sweep: None,
        }
    }
}
pub fn linear(min: f64, max: f64, i: u32, count: u32) -> f64 {
    if count == 1 {
        min
    } else {
        min + (max - min) * i as f64 / (count - 1) as f64
    }
}
impl Curves {
    pub fn validate(&self) -> Result<()> {
        self.physics.validate()?;
        bounded(self.radius_min_kpc, 0.01, 1000.0, "radius_min_kpc")?;
        bounded(
            self.radius_max_kpc,
            self.radius_min_kpc,
            1000.0,
            "radius_max_kpc",
        )?;
        if self.models.is_empty()
            || self.models.len() > 5
            || self.radii == 0
            || self.radii as usize > MAX_CASES
        {
            return Err("Choose 1..=5 models and 1..=1048576 radii".into());
        }
        let count = self.sweep.as_ref().map_or(1, |s| s.count);
        if count == 0
            || count as usize > MAX_CASES
            || self.models.len() as u64 * count as u64 * self.radii as u64 > MAX_CASES as u64
        {
            return Err("Curve jobs must contain 1..=1048576 evaluations".into());
        }
        if let Some(s) = &self.sweep {
            if !s.min.is_finite() || !s.max.is_finite() || s.max < s.min {
                return Err("Invalid sweep interval".into());
            }
            for value in [s.min, s.max] {
                let mut p = self.physics.clone();
                s.parameter.set(&mut p, value);
                p.validate()?;
            }
        }
        Ok(())
    }
    pub fn cases(&self) -> Vec<(f64, f64, Physics)> {
        let mut result = Vec::new();
        for model in &self.models {
            let n = self.sweep.as_ref().map_or(1, |s| s.count);
            for i in 0..n {
                let mut p = self.physics.clone();
                p.model = *model;
                let value = self
                    .sweep
                    .as_ref()
                    .map_or(0.0, |s| linear(s.min, s.max, i, n));
                if let Some(s) = &self.sweep {
                    s.parameter.set(&mut p, value);
                }
                for j in 0..self.radii {
                    result.push((
                        linear(self.radius_min_kpc, self.radius_max_kpc, j, self.radii),
                        value,
                        p.clone(),
                    ));
                }
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Compact {
    pub mass_min_msun: f64,
    pub mass_max_msun: f64,
    pub masses: u32,
    pub spin_min: f64,
    pub spin_max: f64,
    pub spins: u32,
    pub velocity_dispersion_kms: f64,
}
impl Default for Compact {
    fn default() -> Self {
        Self {
            mass_min_msun: 1e4,
            mass_max_msun: 1e10,
            masses: 128,
            spin_min: -0.998,
            spin_max: 0.998,
            spins: 128,
            velocity_dispersion_kms: 150.0,
        }
    }
}
impl Compact {
    pub fn validate(&self) -> Result<()> {
        bounded(self.mass_min_msun, 1.0, 1e11, "mass_min_msun")?;
        bounded(
            self.mass_max_msun,
            self.mass_min_msun,
            1e11,
            "mass_max_msun",
        )?;
        bounded(self.spin_min, -0.998, 0.998, "spin_min")?;
        bounded(self.spin_max, self.spin_min, 0.998, "spin_max")?;
        bounded(
            self.velocity_dispersion_kms,
            1.0,
            1000.0,
            "velocity_dispersion_kms",
        )?;
        if self.masses == 0
            || self.spins == 0
            || self.masses as u64 * self.spins as u64 > MAX_CASES as u64
        {
            return Err("Compact jobs must contain 1..=1048576 evaluations".into());
        }
        Ok(())
    }
    pub fn cases(&self) -> Vec<(f64, f64)> {
        (0..self.masses)
            .flat_map(|i| {
                (0..self.spins).map(move |j| {
                    (
                        10.0_f64.powf(linear(
                            self.mass_min_msun.log10(),
                            self.mass_max_msun.log10(),
                            i,
                            self.masses,
                        )),
                        linear(self.spin_min, self.spin_max, j, self.spins),
                    )
                })
            })
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Task {
    Spin(Spin),
    Curves(Curves),
    Compact(Compact),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub schema_version: u32,
    pub task: Task,
}
impl Job {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err("Unsupported job schema_version; expected 1".into());
        }
        match &self.task {
            Task::Spin(s) => s.validate(),
            Task::Curves(c) => c.validate(),
            Task::Compact(c) => c.validate(),
        }
    }
}
