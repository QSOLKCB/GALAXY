// SPDX-License-Identifier: Apache-2.0
//! Memory-bounded 64-bit tiled execution for GALAXY spin jobs.
//!
//! This binary deliberately lives beside the v0.3 runtime while the wider-than-u32
//! execution contract is validated.  Particles remain independent test particles in
//! the same fixed UFF-derived potential, so partitioning them into resident tiles
//! changes the execution schedule, not the equations of motion.

use bytemuck::{Pod, Zeroable};
use clap::{Args, Parser, Subcommand};
use galaxy_runtime::{
    config::{bounded, Integrator, Physics, Result, Spin as LegacySpin},
    output,
    reference::{self, Particle, KMS_TO_KPC_MYR},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Instant,
};
use wgpu::util::DeviceExt;

const MAX_TILE_PARTICLES: u32 = 8_388_608;
const MAX_SNAPSHOTS: u32 = 256;
const BACKENDS: wgpu::Backends = wgpu::Backends::from_bits_retain(
    wgpu::Backends::VULKAN.bits() | wgpu::Backends::METAL.bits() | wgpu::Backends::DX12.bits(),
);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct U64Spin {
    physics: Physics,
    integrator: Integrator,
    logical_particles: u64,
    tile_particles: u32,
    seed: u32,
    steps: u32,
    dt_myr: f64,
    snapshot_every: u32,
    snapshot_limit: u32,
    arms: u32,
    pitch_deg: f64,
    scatter: f64,
    bulge_fraction: f64,
    thickness_kpc: f64,
    direction: i32,
    radial_kick_kms: f64,
    softening_kpc: f64,
    image_size: u32,
    extent_kpc: f64,
    inclination_deg: f64,
}

impl Default for U64Spin {
    fn default() -> Self {
        let s = LegacySpin::default();
        Self {
            physics: s.physics,
            integrator: s.integrator,
            logical_particles: s.particles as u64,
            tile_particles: MAX_TILE_PARTICLES,
            seed: s.seed,
            steps: s.steps,
            dt_myr: s.dt_myr,
            snapshot_every: s.snapshot_every,
            snapshot_limit: s.snapshot_limit,
            arms: s.arms,
            pitch_deg: s.pitch_deg,
            scatter: s.scatter,
            bulge_fraction: s.bulge_fraction,
            thickness_kpc: s.thickness_kpc,
            direction: s.direction,
            radial_kick_kms: s.radial_kick_kms,
            softening_kpc: s.softening_kpc,
            image_size: s.image_size,
            extent_kpc: s.extent_kpc,
            inclination_deg: s.inclination_deg,
        }
    }
}

impl U64Spin {
    fn validate(&self) -> Result<()> {
        self.physics.validate()?;
        if self.logical_particles == 0 {
            return Err("logical_particles must be greater than zero".into());
        }
        if self.tile_particles == 0 || self.tile_particles > MAX_TILE_PARTICLES {
            return Err(format!("tile_particles must be in 1..={MAX_TILE_PARTICLES}").into());
        }
        if self.steps == 0 || self.steps > 100_000 {
            return Err("steps must be in 1..=100000".into());
        }
        if self.snapshot_limit == 0 || self.snapshot_limit > 65_536 {
            return Err("snapshot_limit must be in 1..=65536".into());
        }
        if self.snapshot_count() > MAX_SNAPSHOTS {
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
        self.particle_updates()
            .ok_or("particle update count exceeds the u64 receipt contract")?;
        self.logical_particles
            .checked_add(self.tile_particles as u64 - 1)
            .ok_or("logical particle/tile range overflows u64")?;
        Ok(())
    }

    fn sample_count(&self) -> u32 {
        self.logical_particles.min(self.snapshot_limit as u64) as u32
    }

    fn tile_count(&self) -> u64 {
        self.logical_particles.div_ceil(self.tile_particles as u64)
    }

    fn snapshot_count(&self) -> u32 {
        if self.snapshot_every == 0 {
            2
        } else {
            1 + self.steps.div_ceil(self.snapshot_every)
        }
    }

    fn frame_steps(&self) -> Vec<u32> {
        if self.snapshot_every == 0 {
            return vec![0, self.steps];
        }
        let mut result = vec![0];
        let mut step = 0;
        while step < self.steps {
            step += self.snapshot_every.min(self.steps - step);
            result.push(step);
        }
        result
    }

    fn sample_ids(&self) -> Vec<u64> {
        let count = self.sample_count();
        (0..count)
            .map(|i| {
                ((i as u128 * self.logical_particles as u128) / count as u128) as u64
            })
            .collect()
    }

    fn particle_updates(&self) -> Option<u64> {
        let multiplier = if self.integrator == Integrator::Circular {
            (self.frame_steps().len() - 1) as u64
        } else {
            self.steps as u64
        };
        self.logical_particles.checked_mul(multiplier)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    schema_version: u32,
    task: U64Spin,
}

impl Job {
    fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err("Unsupported galaxy-u64 schema_version; expected 1".into());
        }
        self.task.validate()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Case {
    info: [u32; 4],
    p0: [f32; 4],
    p1: [f32; 4],
    p2: [f32; 4],
}

impl Case {
    fn sample(local_index: u32, p: &Physics) -> Self {
        Self {
            info: [p.model.id(), local_index, 0, 0],
            p0: [
                p.disk_ml as f32,
                p.bulge_ml as f32,
                p.black_hole_million as f32,
                p.uff_v_inf as f32,
            ],
            p1: [
                p.uff_core as f32,
                p.uff_beta as f32,
                p.halo_log_mass as f32,
                p.halo_concentration as f32,
            ],
            p2: [
                p.burkert_log_density as f32,
                p.burkert_core as f32,
                p.mond_a0 as f32,
                1.0,
            ],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Global {
    info: [u32; 4],
    motion: [f32; 4],
    shape: [f32; 4],
    extra: [f32; 4],
    phase_time: [f32; 4],
    address: [u32; 4],
}

fn split_u64(value: u64) -> [u32; 2] {
    [value as u32, (value >> 32) as u32]
}

fn phase_time_parts(time_myr: f64) -> [f32; 4] {
    let mut remaining = time_myr / std::f64::consts::TAU;
    let mut parts = [0.0; 4];
    for part in &mut parts {
        let high = f64::from_bits(remaining.to_bits() & !((1_u64 << 41) - 1));
        *part = high as f32;
        remaining -= high;
    }
    parts
}

impl Global {
    fn spin(s: &U64Spin, step: u32, tile_base: u64, tile_count: u32, samples: u32) -> Self {
        let base = split_u64(tile_base);
        let total = split_u64(s.logical_particles);
        Self {
            info: [tile_count, s.seed, step, samples],
            motion: [
                s.dt_myr as f32,
                (step as f64 * s.dt_myr) as f32,
                s.direction as f32,
                s.softening_kpc as f32,
            ],
            shape: [
                s.arms as f32,
                s.pitch_deg.to_radians() as f32,
                s.scatter as f32,
                s.bulge_fraction as f32,
            ],
            extra: [
                s.thickness_kpc as f32,
                s.radial_kick_kms as f32,
                if s.integrator == Integrator::Leapfrog { 1.0 } else { 0.0 },
                0.0,
            ],
            phase_time: phase_time_parts(step as f64 * s.dt_myr),
            address: [base[0], base[1], total[0], total[1]],
        }
    }
}

fn instance() -> wgpu::Instance {
    wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: BACKENDS,
        ..Default::default()
    })
}

fn software(info: &wgpu::AdapterInfo) -> bool {
    let name = info.name.to_lowercase();
    info.device_type == wgpu::DeviceType::Cpu
        || ["llvmpipe", "lavapipe", "swiftshader", "software"]
            .iter()
            .any(|s| name.contains(s))
}

fn describe(adapter: &wgpu::Adapter) -> Value {
    let info = adapter.get_info();
    let limits = adapter.limits();
    json!({
        "name": info.name,
        "backend": format!("{:?}", info.backend),
        "device_type": format!("{:?}", info.device_type),
        "driver": info.driver,
        "driver_info": info.driver_info,
        "software": software(&info),
        "max_storage_buffer_bytes": limits.max_storage_buffer_binding_size,
        "max_buffer_bytes": limits.max_buffer_size
    })
}

fn adapters() -> Vec<wgpu::Adapter> {
    let mut adapters = instance().enumerate_adapters(BACKENDS);
    adapters.sort_by_key(|a| match a.get_info().device_type {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        _ => 3,
    });
    adapters
}

fn devices() -> Vec<Value> {
    adapters()
        .iter()
        .enumerate()
        .map(|(index, a)| {
            let mut info = describe(a);
            info["index"] = json!(index);
            info
        })
        .collect()
}

fn select_adapter(infos: &[wgpu::AdapterInfo], name: Option<&str>, allow_software: bool) -> Option<usize> {
    if let Some(index) = name
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|&i| i < infos.len())
    {
        return (allow_software || !software(&infos[index])).then_some(index);
    }
    let query = name.map(str::to_lowercase);
    infos.iter().position(|info| {
        (allow_software || !software(info))
            && query
                .as_ref()
                .map_or(true, |n| info.name.to_lowercase().contains(n))
    })
}

struct Gpu {
    info: Value,
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipelines: HashMap<&'static str, wgpu::ComputePipeline>,
}

struct Field {
    global: wgpu::Buffer,
    bind: wgpu::BindGroup,
    output: wgpu::Buffer,
    sample_count: u32,
    tile_base: u64,
    tile_count: u32,
}

impl Gpu {
    fn new(name: Option<&str>, allow_software: bool) -> Result<Self> {
        let mut available = adapters();
        let infos: Vec<_> = available.iter().map(|a| a.get_info()).collect();
        let index = select_adapter(&infos, name, allow_software).ok_or(
            "No matching hardware compute adapter. Run `devices`; --allow-software permits software Vulkan only for validation.",
        )?;
        let adapter = available.swap_remove(index);
        let mut info = describe(&adapter);
        info["index"] = json!(index);
        let available_limits = adapter.limits();
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: available_limits.max_storage_buffer_binding_size,
            max_buffer_size: available_limits.max_buffer_size,
            ..wgpu::Limits::default()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("GALAXY u64 tiled compute"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::MemoryUsage,
            },
            None,
        ))?;
        let entries: Vec<_> = (0..4)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 0 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: binding == 1,
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GALAXY u64 bindings"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("GALAXY u64 pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let source = format!(
            "{}\n{}",
            include_str!(concat!(env!("OUT_DIR"), "/uff-data.wgsl")),
            include_str!("u64_kernels.wgsl")
        );
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GALAXY u64 compute kernels"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let mut pipelines = HashMap::new();
        for entry in ["initialize", "circular", "leapfrog", "gather"] {
            pipelines.insert(
                entry,
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                }),
            );
        }
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("GPU shader validation: {error}").into());
        }
        Ok(Self {
            info,
            device,
            queue,
            layout,
            pipelines,
        })
    }

    fn checked_buffer(&self, count: u64, stride: u64) -> Result<u64> {
        let size = count.checked_mul(stride).ok_or("Buffer size overflow")?;
        let limits = self.device.limits();
        if count == 0
            || size > limits.max_buffer_size
            || size > limits.max_storage_buffer_binding_size as u64
        {
            return Err(format!(
                "Requested {size} bytes exceeds this adapter's storage-buffer capacity; reduce tile_particles"
            )
            .into());
        }
        Ok(size)
    }

    fn initialize_tile(
        &self,
        s: &U64Spin,
        tile_base: u64,
        tile_count: u32,
        sample_indices: &[u32],
    ) -> Result<Field> {
        if tile_count == 0
            || tile_count > s.tile_particles
            || tile_base.checked_add(tile_count as u64).is_none()
            || tile_base + tile_count as u64 > s.logical_particles
            || sample_indices.iter().any(|&i| i >= tile_count)
        {
            return Err("Invalid resident tile or tile-local sample index".into());
        }
        let mut cases: Vec<Case> = sample_indices
            .iter()
            .map(|&i| Case::sample(i, &s.physics))
            .collect();
        if cases.is_empty() {
            cases.push(Case::sample(0, &s.physics));
        }
        self.checked_buffer(cases.len() as u64, 64)?;
        let state_bytes = self.checked_buffer(tile_count as u64, 32)?;
        let output_count = (sample_indices.len() as u64).max(1);
        let output_bytes = self.checked_buffer(output_count, 32)?;
        let global_value = Global::spin(s, 0, tile_base, tile_count, sample_indices.len() as u32);
        let global = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("u64 job parameters"),
            contents: bytemuck::bytes_of(&global_value),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let cases = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("u64 sample map and UFF case"),
            contents: bytemuck::cast_slice(&cases),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let particles = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("u64 resident particle tile"),
            size: state_bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let output = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("u64 bounded sample output"),
            size: output_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("u64 tile bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: global.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: cases.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: particles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        let field = Field {
            global,
            bind,
            output,
            sample_count: sample_indices.len() as u32,
            tile_base,
            tile_count,
        };
        self.dispatch(&field, "initialize", tile_count, 1);
        Ok(field)
    }

    fn dispatch(&self, field: &Field, kernel: &str, count: u32, repeats: u32) {
        for start in (0..repeats).step_by(256) {
            let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some(kernel),
            });
            for _ in start..repeats.min(start + 256) {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(kernel),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipelines[kernel]);
                pass.set_bind_group(0, &field.bind, &[]);
                pass.dispatch_workgroups(count.div_ceil(256), 1, 1);
            }
            self.queue.submit(Some(encoder.finish()));
            self.device.poll(wgpu::Maintain::Wait);
        }
    }

    fn advance(&self, field: &Field, s: &U64Spin, steps: u32, total: u32) {
        self.queue.write_buffer(
            &field.global,
            0,
            bytemuck::bytes_of(&Global::spin(
                s,
                total,
                field.tile_base,
                field.tile_count,
                field.sample_count,
            )),
        );
        self.dispatch(
            field,
            if s.integrator == Integrator::Circular {
                "circular"
            } else {
                "leapfrog"
            },
            field.tile_count,
            if s.integrator == Integrator::Circular { 1 } else { steps },
        );
    }

    fn sample(&self, field: &Field) -> Result<Vec<Particle>> {
        if field.sample_count == 0 {
            return Ok(Vec::new());
        }
        self.dispatch(field, "gather", field.sample_count, 1);
        let size = field.sample_count as u64 * 32;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("u64 sample readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&field.output, 0, &buffer, 0, size);
        self.queue.submit(Some(encoder.finish()));
        let slice = buffer.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let view = slice.get_mapped_range();
        let result = bytemuck::cast_slice::<u8, Particle>(&view).to_vec();
        drop(view);
        buffer.unmap();
        if result
            .iter()
            .any(|p| p.orbit.iter().chain(&p.state).any(|v| !v.is_finite()))
        {
            return Err("GPU produced a non-finite tiled result; reduce dt or check parameters".into());
        }
        Ok(result)
    }
}

fn hash32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^ (x >> 16)
}

fn address_word(index: u64, seed: u32, lane: u32) -> u32 {
    let lo = index as u32;
    let hi = (index >> 32) as u32;
    let lane_key = (lane + 1).wrapping_mul(0x9e3779b9);
    let low_mix = hash32(lo ^ seed ^ lane_key ^ hash32(hi ^ 0xa511e9b3));
    hash32(low_mix ^ hi.wrapping_mul(0x9e3779b9) ^ 0x85ebca6b)
}

fn random_global(index: u64, seed: u32, lane: u32) -> f64 {
    (address_word(index, seed, lane) >> 8) as f64 / 16_777_216.0
}

fn address_fingerprint(index: u64, seed: u32) -> String {
    (0..5)
        .map(|lane| format!("{:08x}", address_word(index, seed, lane)))
        .collect::<Vec<_>>()
        .join("")
}

fn initialize_particle(s: &U64Spin, index: u64) -> Particle {
    let u = random_global(index, s.seed, 0);
    let v = random_global(index, s.seed, 1);
    let w = random_global(index, s.seed, 2);
    let kind = random_global(index, s.seed, 4);
    let bulge = kind < s.bulge_fraction as f32 as f64;
    let halo = kind >= 0.97;
    let r = 12.0 * galaxy_sampler::physics::star_radius(u, kind, s.bulge_fraction);
    let theta = if bulge || halo {
        std::f64::consts::TAU * v
    } else {
        (v * s.arms as f64).floor() * std::f64::consts::TAU / s.arms as f64
            + (r / 1.2).ln() / s.pitch_deg.to_radians().tan()
            + (w - 0.5) * s.scatter
    };
    let z = (random_global(index, s.seed, 3) - 0.5)
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
        reference::velocity(soft_r, &s.physics) * r / soft_r
    } else {
        reference::velocity(r, &s.physics)
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
}

fn write_snapshot(
    directory: &Path,
    s: &U64Spin,
    step: u32,
    ids: &[u64],
    points: &[Particle],
) -> Result<Value> {
    if ids.len() != points.len()
        || points.len() != s.sample_count() as usize
        || points
            .iter()
            .any(|p| p.state.iter().chain(&p.orbit).any(|v| !v.is_finite()))
    {
        return Err("Invalid u64 snapshot values, ids, or sample length".into());
    }
    let stem = format!("frame-{step:06}");
    let mut writer = BufWriter::new(File::create(directory.join(format!("{stem}.csv")))?);
    writeln!(
        writer,
        "particle_id,x_kpc,y_kpc,z_kpc,vx_kms,vy_kms,initial_radius_kpc"
    )?;
    let mut max_radius: f64 = 0.0;
    let mut max_lz_drift: f64 = 0.0;
    for (id, p) in ids.iter().zip(points) {
        let [x, y, vx, vy] = p.state.map(f64::from);
        let initial_lz = p.orbit[0] as f64 * p.orbit[0] as f64 * p.orbit[3] as f64;
        max_radius = max_radius.max(x.hypot(y));
        if initial_lz != 0.0 {
            max_lz_drift = max_lz_drift.max(((x * vy - y * vx - initial_lz) / initial_lz).abs());
        }
        writeln!(
            writer,
            "{id},{x:.9},{y:.9},{:.9},{:.9},{:.9},{:.9}",
            p.orbit[2],
            vx / KMS_TO_KPC_MYR,
            vy / KMS_TO_KPC_MYR,
            p.orbit[0]
        )?;
    }
    writer.flush()?;

    let size = s.image_size as usize;
    let mut light = vec![0.0_f32; size * size];
    let tilt = s.inclination_deg.to_radians();
    for p in points {
        let x = p.state[0] as f64;
        let y = p.state[1] as f64 * tilt.cos() + p.orbit[2] as f64 * tilt.sin();
        let px = ((x / s.extent_kpc + 1.0) * 0.5 * size as f64) as i64;
        let py = ((y / s.extent_kpc + 1.0) * 0.5 * size as f64) as i64;
        for dy in -1_i64..=1 {
            for dx in -1_i64..=1 {
                let xx = px + dx;
                let yy = py + dy;
                if xx >= 0 && yy >= 0 && xx < size as i64 && yy < size as i64 {
                    light[yy as usize * size + xx as usize] +=
                        if dx == 0 && dy == 0 { 0.7 } else { 0.12 };
                }
            }
        }
    }
    let mut pixels = Vec::with_capacity(size * size * 3);
    for value in light {
        let value = 1.0 - (-value).exp();
        pixels.extend([
            (3.0 + 237.0 * value) as u8,
            (5.0 + 211.0 * value) as u8,
            (9.0 + 166.0 * value) as u8,
        ]);
    }
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(directory.join(format!("{stem}.png")))?),
        s.image_size,
        s.image_size,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png = encoder.write_header()?;
    png.write_image_data(&pixels)?;
    png.finish()?;

    Ok(json!({
        "step": step,
        "time_myr": step as f64 * s.dt_myr,
        "image": format!("{stem}.png"),
        "csv": format!("{stem}.csv"),
        "sampled_particles": points.len(),
        "max_sampled_radius_kpc": max_radius,
        "max_sampled_relative_lz_drift": max_lz_drift
    }))
}

fn viewer(directory: &Path, frames: &[Value], particles: u64) -> Result<()> {
    let data = serde_json::to_string(frames)?;
    let html = format!(
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>GALAXY u64 tiled run</title>
<style>body{{margin:0;background:#080a0d;color:#e6e4df;font:15px system-ui;max-width:1100px;padding:24px;margin:auto}}h1{{font-weight:500;letter-spacing:.12em}}img{{display:block;width:min(100%,850px);margin:20px auto}}input{{width:70%;accent-color:#d7b47c}}button{{padding:8px 16px;background:#292219;color:#efcca0;border:1px solid #745b35;cursor:pointer}}p{{color:#a2a6ae}}a{{color:#d7b47c}}</style>
<h1>GALAXY / U64 TILED RUN</h1><p>{particles} logical particles · bounded resident tiles · preview shows the globally sampled export</p><img id="frame" alt="Galaxy particle positions"><button id="play">Play</button> <input id="step" aria-label="Snapshot" type="range" min="0" max="{}" value="0"><p id="caption"></p><a href="receipt.json">Run report</a> · <a href="job.json">Resolved job</a>
<script>const frames={data};const slider=document.getElementById('step');let timer;function show(){{const f=frames[Number(slider.value)];document.getElementById('frame').src=f.image;document.getElementById('caption').textContent='Step '+f.step+' · '+f.time_myr.toFixed(2)+' Myr · '+f.sampled_particles+' globally sampled particles';}}slider.oninput=show;document.getElementById('play').onclick=function(){{if(timer){{clearInterval(timer);timer=null;this.textContent='Play';}}else{{this.textContent='Pause';timer=setInterval(()=>{{slider.value=(Number(slider.value)+1)%frames.length;show();}},250);}}}};show();</script></html>"#,
        frames.len().saturating_sub(1)
    );
    fs::write(directory.join("viewer.html"), html)?;
    Ok(())
}

fn load_job(path: &Path) -> Result<Job> {
    if fs::metadata(path)?.len() > 65_536 {
        return Err("Job JSON must be at most 64 KiB".into());
    }
    let job: Job = serde_json::from_slice(&fs::read(path)?)?;
    job.validate()?;
    Ok(job)
}

fn execute(job: &Job, directory: &Path, gpu: &Gpu) -> Result<Value> {
    let s = &job.task;
    let started = Instant::now();
    let frame_steps = s.frame_steps();
    let sample_ids = s.sample_ids();
    let sample_count = sample_ids.len();
    let mut cache: Vec<Vec<Option<Particle>>> =
        vec![vec![None; sample_count]; frame_steps.len()];
    let tiles = s.tile_count();
    let mut sample_cursor = 0usize;
    let mut initialization_seconds = 0.0;
    let mut integration_seconds = 0.0;
    let progress_every = (tiles / 20).max(1);

    for tile in 0..tiles {
        let base = tile
            .checked_mul(s.tile_particles as u64)
            .ok_or("tile base overflow")?;
        let count = (s.logical_particles - base).min(s.tile_particles as u64) as u32;
        let end = base + count as u64;
        let mut local_indices = Vec::new();
        let mut slots = Vec::new();
        while sample_cursor < sample_ids.len() && sample_ids[sample_cursor] < end {
            let id = sample_ids[sample_cursor];
            if id < base {
                return Err("global sample plan moved backwards across a tile".into());
            }
            local_indices.push((id - base) as u32);
            slots.push(sample_cursor);
            sample_cursor += 1;
        }

        let init = Instant::now();
        let field = gpu.initialize_tile(s, base, count, &local_indices)?;
        initialization_seconds += init.elapsed().as_secs_f64();
        let mut previous = 0;
        for (frame_index, &step) in frame_steps.iter().enumerate() {
            if step > previous {
                let advance = Instant::now();
                gpu.advance(&field, s, step - previous, step);
                integration_seconds += advance.elapsed().as_secs_f64();
                previous = step;
            }
            if !local_indices.is_empty() {
                let points = gpu.sample(&field)?;
                if points.len() != slots.len() {
                    return Err("tile gather returned the wrong number of global samples".into());
                }
                for (&slot, point) in slots.iter().zip(points) {
                    cache[frame_index][slot] = Some(point);
                }
            }
        }

        if tile < 2 || tile + 1 == tiles || (tile + 1) % progress_every == 0 {
            eprintln!(
                "tile {}/{} · global ids {}..{} · {} resident · {} global samples",
                tile + 1,
                tiles,
                base,
                end - 1,
                count,
                local_indices.len()
            );
        }
    }

    if sample_cursor != sample_ids.len() {
        return Err("not every planned global sample was assigned to a resident tile".into());
    }

    let mut frames = Vec::new();
    for (frame_index, &step) in frame_steps.iter().enumerate() {
        let points: Vec<Particle> = cache[frame_index]
            .drain(..)
            .map(|p| p.ok_or("missing global sample after tiled execution"))
            .collect::<std::result::Result<_, _>>()?;
        frames.push(write_snapshot(directory, s, step, &sample_ids, &points)?);
        eprintln!(
            "snapshot {}/{} · step {}/{} · {:.2} Myr · {} globally sampled",
            frame_index + 1,
            frame_steps.len(),
            step,
            s.steps,
            step as f64 * s.dt_myr,
            points.len()
        );
    }
    viewer(directory, &frames, s.logical_particles)?;

    let updates = s.particle_updates().expect("validated update count");
    let throughput = if integration_seconds > 0.0 {
        updates as f64 / integration_seconds
    } else {
        0.0
    };
    Ok(json!({
        "adapter": gpu.info,
        "arithmetic": "float32 GPU kernels; 64-bit split global addressing on CPU/WGSL; float64 host diagnostics",
        "addressing": "split-u64-hash32-avalanche-v1",
        "logical_particles": s.logical_particles,
        "resident_tile_particles": (s.logical_particles.min(s.tile_particles as u64)),
        "resident_particle_bytes": s.logical_particles.min(s.tile_particles as u64) * 32,
        "tiles": tiles,
        "snapshot_limit": s.sample_count(),
        "sample_index_math": "exact u128 host mapping into the u64 logical index space",
        "sample_cache_bytes": frame_steps.len() as u64 * sample_count as u64 * 32,
        "simulated_time_myr": s.steps as f64 * s.dt_myr,
        "particle_updates": updates,
        "initialization_compute_wall_seconds": initialization_seconds,
        "integration_compute_wall_seconds": integration_seconds,
        "integration_particle_updates_per_second": throughput,
        "execution_wall_seconds": started.elapsed().as_secs_f64(),
        "frames": frames,
        "force_model": if s.integrator == Integrator::Circular {
            "prescribed UFF circular speed"
        } else {
            "planar fixed UFF potential; softened radial force; static authored height"
        },
        "partition_semantics": "independent test particles in one fixed potential; tiles are an execution partition, not a physics approximation"
    }))
}

fn verify_u64(gpu: Option<&Gpu>) -> Result<Value> {
    let ids = [(1_u64 << 32) - 1, 1_u64 << 32, (1_u64 << 32) + 1];
    let fingerprints: Vec<String> = ids
        .iter()
        .map(|&id| address_fingerprint(id, 303))
        .collect();
    if fingerprints[0] == fingerprints[1]
        || fingerprints[1] == fingerprints[2]
        || fingerprints[0] == fingerprints[2]
    {
        return Err("64-bit addressing aliases at the 2^32 boundary".into());
    }

    let mut gpu_checked = false;
    if let Some(gpu) = gpu {
        let s = U64Spin {
            logical_particles: (1_u64 << 32) + 2,
            tile_particles: 4,
            snapshot_limit: 3,
            steps: 1,
            ..U64Spin::default()
        };
        s.validate()?;
        let base = (1_u64 << 32) - 1;
        let field = gpu.initialize_tile(&s, base, 3, &[0, 1, 2])?;
        let actual = gpu.sample(&field)?;
        for (result, &id) in actual.iter().zip(&ids) {
            let expected = initialize_particle(&s, id);
            for (a, b) in result
                .orbit
                .iter()
                .chain(&result.state)
                .zip(expected.orbit.iter().chain(&expected.state))
            {
                if !a.is_finite() || (*a - *b).abs() > 4e-4 * (1.0 + b.abs()) {
                    return Err(format!(
                        "GPU u64 boundary initialization mismatch at id {id}: {a} vs {b}"
                    )
                    .into());
                }
            }
        }
        gpu_checked = true;
    }

    Ok(json!({
        "status": "passed",
        "addressing": "split-u64-hash32-avalanche-v1",
        "boundary": "2^32",
        "boundary_ids": ids,
        "address_fingerprints": fingerprints,
        "distinct_boundary_addresses": true,
        "gpu_boundary_checked": gpu_checked,
        "adapter": gpu.map(|g| &g.info)
    }))
}

fn run(job_path: &Path, directory: &Path, adapter: Option<&str>, allow_software: bool) -> Result<()> {
    let job = load_job(job_path)?;
    if let Some(parent) = directory.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(directory).map_err(|e| {
        format!(
            "Output directory must be new ({}): {e}",
            directory.display()
        )
    })?;
    output::write_json(&directory.join("job.json"), &job)?;
    let provenance: Value = serde_json::from_str(include_str!("../../../data/uff/provenance.json"))?;
    let mut receipt = json!({
        "schema_version": 1,
        "runtime": "galaxy-u64",
        "runtime_package_version": env!("CARGO_PKG_VERSION"),
        "status": "running",
        "runtime_source_sha256": env!("GALAXY_RUNTIME_SOURCE_SHA256"),
        "job_sha256": output::sha256(&fs::read(directory.join("job.json"))?),
        "uff_source": provenance,
        "backend_requested": "gpu",
        "allow_software": allow_software
    });
    output::write_json(&directory.join("receipt.json"), &receipt)?;

    let result = (|| {
        let gpu = Gpu::new(adapter, allow_software)?;
        eprintln!("Engine: {} ({})", gpu.info["name"], gpu.info["backend"]);
        let details = execute(&job, directory, &gpu)?;
        receipt["results"] = details;
        receipt["artifacts"] = json!(output::artifacts(directory)?);
        receipt["status"] = json!("complete");
        output::write_json(&directory.join("receipt.json"), &receipt)
    })();
    if let Err(error) = result {
        receipt["status"] = json!("failed");
        receipt["error"] = json!(error.to_string());
        output::write_json(&directory.join("receipt.json"), &receipt)?;
        return Err(error);
    }
    println!("Completed: {}", directory.display());
    Ok(())
}

#[derive(Parser)]
#[command(version, about = "GALAXY memory-bounded 64-bit tiled GPU runtime")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct Backend {
    #[arg(long, help = "Adapter index from devices, or a case-insensitive name substring")]
    adapter: Option<String>,
    #[arg(long, help = "Permit software Vulkan for validation; never implied")]
    allow_software: bool,
}

#[derive(Subcommand)]
enum Command {
    /// List native compute adapters.
    Devices,
    /// Validate and print a resolved u64 tiled job.
    Validate {
        #[arg(long)]
        job: PathBuf,
    },
    /// Prove distinct addressing across 2^32, optionally on a GPU.
    Verify {
        #[arg(long, help = "Run only the host-side boundary discriminator")]
        cpu: bool,
        #[command(flatten)]
        backend: Backend,
    },
    /// Execute one logical population through bounded resident GPU tiles.
    Run {
        #[arg(long)]
        job: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        backend: Backend,
    },
}

fn main() {
    let result: Result<()> = (|| {
        match Cli::parse().command {
            Command::Devices => println!("{}", serde_json::to_string_pretty(&devices())?),
            Command::Validate { job } => {
                println!("{}", serde_json::to_string_pretty(&load_job(&job)?)?)
            }
            Command::Verify { cpu, backend } => {
                let gpu = if cpu {
                    None
                } else {
                    Some(Gpu::new(
                        backend.adapter.as_deref(),
                        backend.allow_software,
                    )?)
                };
                println!("{}", serde_json::to_string_pretty(&verify_u64(gpu.as_ref())?)?);
            }
            Command::Run {
                job,
                output: directory,
                backend,
            } => run(
                &job,
                &directory,
                backend.adapter.as_deref(),
                backend.allow_software,
            )?,
        }
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("GALAXY-U64: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_sampling_crosses_u32_without_aliasing() {
        let s = U64Spin {
            logical_particles: (1_u64 << 32) + MAX_TILE_PARTICLES as u64,
            snapshot_limit: 65_536,
            ..U64Spin::default()
        };
        s.validate().unwrap();
        let ids = s.sample_ids();
        assert_eq!(ids.len(), 65_536);
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        assert!(ids.iter().any(|&id| id >= 1_u64 << 32));
        assert_eq!(s.tile_count(), 513);
    }

    #[test]
    fn boundary_fingerprints_are_distinct_and_stable() {
        let ids = [(1_u64 << 32) - 1, 1_u64 << 32, (1_u64 << 32) + 1];
        let values: Vec<_> = ids.iter().map(|&id| address_fingerprint(id, 303)).collect();
        assert_ne!(values[0], values[1]);
        assert_ne!(values[1], values[2]);
        assert_ne!(values[0], values[2]);
        assert_eq!(values, ids.iter().map(|&id| address_fingerprint(id, 303)).collect::<Vec<_>>());
    }

    #[test]
    fn update_count_and_sample_math_fail_closed() {
        let invalid = U64Spin {
            logical_particles: u64::MAX,
            steps: 100_000,
            integrator: Integrator::Leapfrog,
            ..U64Spin::default()
        };
        assert!(invalid.validate().is_err());
        let valid = U64Spin {
            logical_particles: (1_u64 << 32) + 1,
            steps: 1000,
            integrator: Integrator::Leapfrog,
            ..U64Spin::default()
        };
        valid.validate().unwrap();
        assert_eq!(valid.particle_updates(), Some(((1_u64 << 32) + 1) * 1000));
    }

    #[test]
    fn global_initialization_does_not_wrap_at_u32() {
        let s = U64Spin::default();
        let a = initialize_particle(&s, 0);
        let b = initialize_particle(&s, 1_u64 << 32);
        assert_ne!(a.orbit, b.orbit);
        assert_ne!(a.state, b.state);
    }

    #[test]
    fn global_uniform_sample_uses_exact_u128_product() {
        let s = U64Spin {
            logical_particles: u64::MAX / 200_000,
            snapshot_limit: 65_536,
            steps: 1,
            ..U64Spin::default()
        };
        s.validate().unwrap();
        let ids = s.sample_ids();
        assert_eq!(ids[0], 0);
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }
}
