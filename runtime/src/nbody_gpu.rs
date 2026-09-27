// SPDX-License-Identifier: Apache-2.0
//! BH #2B1 GPU transfer ABI and Vulkan/WGSL traversal.
//!
//! Tree construction remains in galaxy-nbody's frozen BH #2A CPU reference.
//! This module converts that tree to explicit f32/u32 transfer records and
//! executes traversal on a compute adapter.

use crate::config::Result;
use bytemuck::{Pod, Zeroable};
use galaxy_nbody::{
    flat::FlatTree,
    Accel, Body, Config,
};
use serde_json::{json, Value};
use std::{sync::mpsc, time::Instant};
use wgpu::util::DeviceExt;

const BACKENDS: wgpu::Backends = wgpu::Backends::from_bits_retain(
    wgpu::Backends::VULKAN.bits() | wgpu::Backends::METAL.bits() | wgpu::Backends::DX12.bits(),
);

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct BhSettings {
    pub info: [u32; 4],
    pub physics: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct GpuBody {
    pub position_mass: [f32; 4],
    pub meta: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq, Eq)]
pub struct GpuEntry {
    pub data: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct GpuCell {
    pub center_half_mass: [f32; 4],
    pub com: [f32; 4],
    pub range_depth: [u32; 4],
    pub children: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
struct GpuAccel {
    value: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct EvolveSettings {
    pub info: [u32; 4],
    pub motion: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct GpuEvolveState {
    pub position_mass: [f32; 4],
    pub velocity: [f32; 4],
}

#[derive(Debug, Default, Clone, Copy)]
pub struct StageTiming {
    pub transfer_seconds: f64,
    pub dispatch_seconds: f64,
    pub readback_seconds: f64,
}

#[derive(Debug)]
pub struct EvolvingState {
    state: wgpu::Buffer,
    acceleration: wgpu::Buffer,
    count: u32,
    bytes: u64,
}

#[derive(Debug)]
pub struct PackedFlat {
    pub settings: BhSettings,
    pub bodies: Vec<GpuBody>,
    pub entries: Vec<GpuEntry>,
    pub cells: Vec<GpuCell>,
}

impl PackedFlat {
    pub fn byte_len(&self) -> u64 {
        std::mem::size_of::<BhSettings>() as u64
            + self.bodies.len() as u64 * std::mem::size_of::<GpuBody>() as u64
            + self.entries.len() as u64 * std::mem::size_of::<GpuEntry>() as u64
            + self.cells.len() as u64 * std::mem::size_of::<GpuCell>() as u64
    }
}

#[derive(Debug)]
pub struct TraversalResult {
    pub accelerations: Vec<Accel>,
    pub gpu_info: Value,
    pub packed_bytes: u64,
    pub transfer_seconds: f64,
    pub dispatch_seconds: f64,
    pub readback_seconds: f64,
}

fn checked_f32(value: f64, name: &str) -> Result<f32> {
    let packed = value as f32;
    if !packed.is_finite() {
        return Err(format!("{name} cannot be represented as finite f32").into());
    }
    Ok(packed)
}

pub fn pack_flat_tree(bodies: &[Body], tree: &FlatTree, config: Config) -> Result<PackedFlat> {
    config.validate()?;
    if bodies.len() != tree.body_count
        || bodies.len() != tree.entries.len()
        || bodies.len() != tree.entry_position_by_body.len()
    {
        return Err("flat tree/body cardinality mismatch".into());
    }
    if tree.cells.is_empty() || tree.root as usize >= tree.cells.len() {
        return Err("flat tree has invalid root".into());
    }
    if bodies.len() > u32::MAX as usize || tree.cells.len() > u32::MAX as usize {
        return Err("GPU Barnes-Hut transfer requires u32-sized body/cell counts".into());
    }

    let mut packed_bodies = Vec::with_capacity(bodies.len());
    for (index, body) in bodies.iter().enumerate() {
        packed_bodies.push(GpuBody {
            position_mass: [
                checked_f32(body.x, "body.x")?,
                checked_f32(body.y, "body.y")?,
                checked_f32(body.mass, "body.mass")?,
                0.0,
            ],
            meta: [tree.entry_position_by_body[index], 0, 0, 0],
        });
    }

    let entries = tree
        .entries
        .iter()
        .map(|entry| GpuEntry {
            data: [entry.code, entry.body_index, 0, 0],
        })
        .collect();

    let mut cells = Vec::with_capacity(tree.cells.len());
    for cell in &tree.cells {
        cells.push(GpuCell {
            center_half_mass: [
                checked_f32(cell.cx, "cell.cx")?,
                checked_f32(cell.cy, "cell.cy")?,
                checked_f32(cell.half, "cell.half")?,
                checked_f32(cell.mass, "cell.mass")?,
            ],
            com: [
                checked_f32(cell.com_x, "cell.com_x")?,
                checked_f32(cell.com_y, "cell.com_y")?,
                0.0,
                0.0,
            ],
            range_depth: [cell.start, cell.end, cell.depth as u32, 0],
            children: cell.children,
        });
    }

    Ok(PackedFlat {
        settings: BhSettings {
            info: [
                bodies.len() as u32,
                tree.cells.len() as u32,
                tree.root,
                0,
            ],
            physics: [
                checked_f32(config.theta, "theta")?,
                checked_f32(config.softening_kpc, "softening_kpc")?,
                checked_f32(config.g, "G")?,
                0.0,
            ],
        },
        bodies: packed_bodies,
        entries,
        cells,
    })
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
            .any(|needle| name.contains(needle))
}

fn adapters() -> Vec<wgpu::Adapter> {
    let mut adapters = instance().enumerate_adapters(BACKENDS);
    adapters.sort_by_key(|adapter| match adapter.get_info().device_type {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        _ => 3,
    });
    adapters
}

fn select_adapter(
    infos: &[wgpu::AdapterInfo],
    name: Option<&str>,
    allow_software: bool,
) -> Option<usize> {
    if let Some(index) = name
        .and_then(|candidate| candidate.parse::<usize>().ok())
        .filter(|&index| index < infos.len())
    {
        return (allow_software || !software(&infos[index])).then_some(index);
    }
    let query = name.map(str::to_lowercase);
    infos.iter().position(|info| {
        (allow_software || !software(info))
            && query
                .as_ref()
                .map_or(true, |needle| info.name.to_lowercase().contains(needle))
    })
}

fn describe(adapter: &wgpu::Adapter, index: usize) -> Value {
    let info = adapter.get_info();
    json!({
        "index": index,
        "name": info.name,
        "backend": format!("{:?}", info.backend),
        "device_type": format!("{:?}", info.device_type),
        "driver": info.driver,
        "driver_info": info.driver_info,
        "software": software(&info)
    })
}

pub struct NbodyGpu {
    info: Value,
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    evolve_layout: wgpu::BindGroupLayout,
    kick_drift_pipeline: wgpu::ComputePipeline,
    final_kick_pipeline: wgpu::ComputePipeline,
}

impl NbodyGpu {
    pub fn new(name: Option<&str>, allow_software: bool) -> Result<Self> {
        let mut available_adapters = adapters();
        let infos: Vec<_> = available_adapters.iter().map(|adapter| adapter.get_info()).collect();
        let index = select_adapter(&infos, name, allow_software).ok_or(
            "No matching hardware compute adapter. --allow-software permits a software adapter for validation.",
        )?;
        let adapter = available_adapters.swap_remove(index);
        let info = describe(&adapter, index);
        let available = adapter.limits();
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: available.max_storage_buffer_binding_size,
            max_buffer_size: available.max_buffer_size,
            ..wgpu::Limits::default()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("GALAXY Barnes-Hut compute"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::MemoryUsage,
            },
            None,
        ))?;

        let entries = [
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ];
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Barnes-Hut bind layout"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Barnes-Hut pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Barnes-Hut flat traversal"),
            source: wgpu::ShaderSource::Wgsl(include_str!("nbody.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("bh_traverse"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("bh_traverse"),
            compilation_options: Default::default(),
            cache: None,
        });
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("Barnes-Hut GPU shader validation: {error}").into());
        }

        let evolve_entries = [
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ];
        let evolve_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Barnes-Hut evolve bind layout"),
            entries: &evolve_entries,
        });
        let evolve_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Barnes-Hut evolve pipeline layout"),
                bind_group_layouts: &[&evolve_layout],
                push_constant_ranges: &[],
            });
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let evolve_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Barnes-Hut evolution kernels"),
            source: wgpu::ShaderSource::Wgsl(include_str!("nbody_evolve.wgsl").into()),
        });
        let kick_drift_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("bh_kick_drift"),
                layout: Some(&evolve_pipeline_layout),
                module: &evolve_shader,
                entry_point: Some("bh_kick_drift"),
                compilation_options: Default::default(),
                cache: None,
            });
        let final_kick_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("bh_final_kick"),
                layout: Some(&evolve_pipeline_layout),
                module: &evolve_shader,
                entry_point: Some("bh_final_kick"),
                compilation_options: Default::default(),
                cache: None,
            });
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("Barnes-Hut GPU evolution shader validation: {error}").into());
        }

        Ok(Self {
            info,
            device,
            queue,
            layout,
            pipeline,
            evolve_layout,
            kick_drift_pipeline,
            final_kick_pipeline,
        })
    }

    fn checked_storage(&self, count: usize, stride: usize, name: &str) -> Result<u64> {
        let bytes = (count as u64)
            .checked_mul(stride as u64)
            .ok_or_else(|| format!("{name} buffer size overflow"))?;
        let limits = self.device.limits();
        if count == 0
            || bytes > limits.max_buffer_size
            || bytes > limits.max_storage_buffer_binding_size as u64
        {
            return Err(format!(
                "{name} requires {bytes} bytes, beyond this adapter's storage-buffer limit"
            )
            .into());
        }
        Ok(bytes)
    }

    pub fn traverse(&self, packed: &PackedFlat) -> Result<TraversalResult> {
        self.checked_storage(
            packed.bodies.len(),
            std::mem::size_of::<GpuBody>(),
            "Barnes-Hut bodies",
        )?;
        self.checked_storage(
            packed.entries.len(),
            std::mem::size_of::<GpuEntry>(),
            "Barnes-Hut entries",
        )?;
        self.checked_storage(
            packed.cells.len(),
            std::mem::size_of::<GpuCell>(),
            "Barnes-Hut cells",
        )?;
        let output_bytes = self.checked_storage(
            packed.bodies.len(),
            std::mem::size_of::<GpuAccel>(),
            "Barnes-Hut accelerations",
        )?;

        let transfer_started = Instant::now();
        let settings = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Barnes-Hut settings"),
            contents: bytemuck::bytes_of(&packed.settings),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bodies = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Barnes-Hut bodies"),
            contents: bytemuck::cast_slice(&packed.bodies),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let entries = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Barnes-Hut Morton entries"),
            contents: bytemuck::cast_slice(&packed.entries),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let cells = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Barnes-Hut flat cells"),
            contents: bytemuck::cast_slice(&packed.cells),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let output = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Barnes-Hut acceleration output"),
            size: output_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        self.device.poll(wgpu::Maintain::Wait);
        let transfer_seconds = transfer_started.elapsed().as_secs_f64();

        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Barnes-Hut bindings"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: settings.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: bodies.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: entries.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: cells.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: output.as_entire_binding(),
                },
            ],
        });

        let dispatch_started = Instant::now();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Barnes-Hut traversal"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Barnes-Hut traversal"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups((packed.bodies.len() as u32).div_ceil(128), 1, 1);
        }
        self.queue.submit(Some(encoder.finish()));
        self.device.poll(wgpu::Maintain::Wait);
        let dispatch_seconds = dispatch_started.elapsed().as_secs_f64();

        let readback_started = Instant::now();
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Barnes-Hut readback"),
            size: output_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, output_bytes);
        self.queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let view = slice.get_mapped_range();
        let gpu_values = bytemuck::cast_slice::<u8, GpuAccel>(&view).to_vec();
        drop(view);
        readback.unmap();
        let readback_seconds = readback_started.elapsed().as_secs_f64();

        let mut accelerations = Vec::with_capacity(gpu_values.len());
        for (index, value) in gpu_values.into_iter().enumerate() {
            if !value.value[0].is_finite() || !value.value[1].is_finite() {
                return Err(format!("GPU produced non-finite acceleration for body {index}").into());
            }
            accelerations.push(Accel {
                ax: value.value[0] as f64,
                ay: value.value[1] as f64,
            });
        }

        Ok(TraversalResult {
            accelerations,
            gpu_info: self.info.clone(),
            packed_bytes: packed.byte_len(),
            transfer_seconds,
            dispatch_seconds,
            readback_seconds,
        })
    }

    pub fn info(&self) -> Value {
        self.info.clone()
    }

    pub fn create_evolving_state(&self, bodies: &[Body]) -> Result<(EvolvingState, StageTiming)> {
        if bodies.len() < 2 || bodies.len() > u32::MAX as usize {
            return Err("GPU evolving state requires 2..=u32::MAX resident bodies".into());
        }
        let state_bytes = self.checked_storage(
            bodies.len(),
            std::mem::size_of::<GpuEvolveState>(),
            "Barnes-Hut evolving state",
        )?;
        let acceleration_bytes = self.checked_storage(
            bodies.len(),
            std::mem::size_of::<GpuAccel>(),
            "Barnes-Hut evolving acceleration",
        )?;
        let mut packed = Vec::with_capacity(bodies.len());
        for (index, body) in bodies.iter().enumerate() {
            if !body.x.is_finite()
                || !body.y.is_finite()
                || !body.vx.is_finite()
                || !body.vy.is_finite()
                || !body.mass.is_finite()
                || body.mass <= 0.0
            {
                return Err(format!("body {index} contains invalid evolving state").into());
            }
            packed.push(GpuEvolveState {
                position_mass: [
                    checked_f32(body.x, "body.x")?,
                    checked_f32(body.y, "body.y")?,
                    checked_f32(body.mass, "body.mass")?,
                    0.0,
                ],
                velocity: [
                    checked_f32(body.vx, "body.vx")?,
                    checked_f32(body.vy, "body.vy")?,
                    0.0,
                    0.0,
                ],
            });
        }

        let started = Instant::now();
        let state = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Barnes-Hut persistent evolving state"),
            contents: bytemuck::cast_slice(&packed),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
        let acceleration = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Barnes-Hut persistent evolving acceleration"),
            size: acceleration_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        self.device.poll(wgpu::Maintain::Wait);
        Ok((
            EvolvingState {
                state,
                acceleration,
                count: bodies.len() as u32,
                bytes: state_bytes + acceleration_bytes,
            },
            StageTiming {
                transfer_seconds: started.elapsed().as_secs_f64(),
                ..StageTiming::default()
            },
        ))
    }

    pub fn force_into(
        &self,
        evolving: &EvolvingState,
        packed: &PackedFlat,
    ) -> Result<StageTiming> {
        if packed.bodies.len() != evolving.count as usize
            || packed.entries.len() != evolving.count as usize
        {
            return Err("Barnes-Hut force/tree count does not match evolving state".into());
        }
        self.checked_storage(
            packed.bodies.len(),
            std::mem::size_of::<GpuBody>(),
            "Barnes-Hut bodies",
        )?;
        self.checked_storage(
            packed.entries.len(),
            std::mem::size_of::<GpuEntry>(),
            "Barnes-Hut entries",
        )?;
        self.checked_storage(
            packed.cells.len(),
            std::mem::size_of::<GpuCell>(),
            "Barnes-Hut cells",
        )?;

        let transfer_started = Instant::now();
        let settings = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Barnes-Hut evolving force settings"),
            contents: bytemuck::bytes_of(&packed.settings),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bodies = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Barnes-Hut evolving force bodies"),
            contents: bytemuck::cast_slice(&packed.bodies),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let entries = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Barnes-Hut evolving force entries"),
            contents: bytemuck::cast_slice(&packed.entries),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let cells = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Barnes-Hut evolving force cells"),
            contents: bytemuck::cast_slice(&packed.cells),
            usage: wgpu::BufferUsages::STORAGE,
        });
        self.device.poll(wgpu::Maintain::Wait);
        let transfer_seconds = transfer_started.elapsed().as_secs_f64();

        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Barnes-Hut evolving force bindings"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: settings.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: bodies.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: entries.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: cells.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: evolving.acceleration.as_entire_binding(),
                },
            ],
        });
        let dispatch_started = Instant::now();
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Barnes-Hut evolving force traversal"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Barnes-Hut evolving force traversal"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(evolving.count.div_ceil(128), 1, 1);
        }
        self.queue.submit(Some(encoder.finish()));
        self.device.poll(wgpu::Maintain::Wait);
        Ok(StageTiming {
            transfer_seconds,
            dispatch_seconds: dispatch_started.elapsed().as_secs_f64(),
            ..StageTiming::default()
        })
    }

    fn integrate_stage(
        &self,
        evolving: &EvolvingState,
        dt_myr: f64,
        final_kick: bool,
    ) -> Result<StageTiming> {
        if !dt_myr.is_finite() || dt_myr <= 0.0 {
            return Err("dt_myr must be positive and finite".into());
        }
        let settings = EvolveSettings {
            info: [evolving.count, 0, 0, 0],
            motion: [checked_f32(dt_myr, "dt_myr")?, 0.0, 0.0, 0.0],
        };
        let transfer_started = Instant::now();
        let settings = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Barnes-Hut evolve settings"),
            contents: bytemuck::bytes_of(&settings),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        self.device.poll(wgpu::Maintain::Wait);
        let transfer_seconds = transfer_started.elapsed().as_secs_f64();

        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Barnes-Hut evolve bindings"),
            layout: &self.evolve_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: settings.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: evolving.state.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: evolving.acceleration.as_entire_binding(),
                },
            ],
        });
        let dispatch_started = Instant::now();
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(if final_kick {
                "Barnes-Hut final kick"
            } else {
                "Barnes-Hut kick/drift"
            }),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(if final_kick {
                    "Barnes-Hut final kick"
                } else {
                    "Barnes-Hut kick/drift"
                }),
                timestamp_writes: None,
            });
            pass.set_pipeline(if final_kick {
                &self.final_kick_pipeline
            } else {
                &self.kick_drift_pipeline
            });
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(evolving.count.div_ceil(128), 1, 1);
        }
        self.queue.submit(Some(encoder.finish()));
        self.device.poll(wgpu::Maintain::Wait);
        Ok(StageTiming {
            transfer_seconds,
            dispatch_seconds: dispatch_started.elapsed().as_secs_f64(),
            ..StageTiming::default()
        })
    }

    pub fn kick_drift(&self, evolving: &EvolvingState, dt_myr: f64) -> Result<StageTiming> {
        self.integrate_stage(evolving, dt_myr, false)
    }

    pub fn final_kick(&self, evolving: &EvolvingState, dt_myr: f64) -> Result<StageTiming> {
        self.integrate_stage(evolving, dt_myr, true)
    }

    pub fn read_evolving_state(&self, evolving: &EvolvingState) -> Result<(Vec<Body>, StageTiming)> {
        let bytes = (evolving.count as u64) * std::mem::size_of::<GpuEvolveState>() as u64;
        let started = Instant::now();
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Barnes-Hut evolving state readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&evolving.state, 0, &readback, 0, bytes);
        self.queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let view = slice.get_mapped_range();
        let values = bytemuck::cast_slice::<u8, GpuEvolveState>(&view).to_vec();
        drop(view);
        readback.unmap();
        let mut bodies = Vec::with_capacity(values.len());
        for (index, value) in values.into_iter().enumerate() {
            let fields = [
                value.position_mass[0],
                value.position_mass[1],
                value.velocity[0],
                value.velocity[1],
                value.position_mass[2],
            ];
            if fields.iter().any(|value| !value.is_finite()) || value.position_mass[2] <= 0.0 {
                return Err(format!("GPU produced invalid evolving body state at {index}").into());
            }
            bodies.push(Body {
                x: value.position_mass[0] as f64,
                y: value.position_mass[1] as f64,
                vx: value.velocity[0] as f64,
                vy: value.velocity[1] as f64,
                mass: value.position_mass[2] as f64,
            });
        }
        Ok((
            bodies,
            StageTiming {
                readback_seconds: started.elapsed().as_secs_f64(),
                ..StageTiming::default()
            },
        ))
    }

    pub fn read_evolving_accelerations(
        &self,
        evolving: &EvolvingState,
    ) -> Result<(Vec<Accel>, StageTiming)> {
        let bytes = (evolving.count as u64) * std::mem::size_of::<GpuAccel>() as u64;
        let started = Instant::now();
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Barnes-Hut evolving acceleration readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&evolving.acceleration, 0, &readback, 0, bytes);
        self.queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let view = slice.get_mapped_range();
        let values = bytemuck::cast_slice::<u8, GpuAccel>(&view).to_vec();
        drop(view);
        readback.unmap();
        let mut accelerations = Vec::with_capacity(values.len());
        for (index, value) in values.into_iter().enumerate() {
            if !value.value[0].is_finite() || !value.value[1].is_finite() {
                return Err(format!("GPU produced invalid evolving acceleration at {index}").into());
            }
            accelerations.push(Accel {
                ax: value.value[0] as f64,
                ay: value.value[1] as f64,
            });
        }
        Ok((
            accelerations,
            StageTiming {
                readback_seconds: started.elapsed().as_secs_f64(),
                ..StageTiming::default()
            },
        ))
    }

    pub fn evolving_buffer_bytes(&self, evolving: &EvolvingState) -> u64 {
        evolving.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use galaxy_nbody::{flat::build_flat_tree, make_disc};

    #[test]
    fn transfer_record_sizes_are_frozen() {
        assert_eq!(std::mem::size_of::<BhSettings>(), 32);
        assert_eq!(std::mem::size_of::<GpuBody>(), 32);
        assert_eq!(std::mem::size_of::<GpuEntry>(), 16);
        assert_eq!(std::mem::size_of::<GpuCell>(), 64);
        assert_eq!(std::mem::size_of::<GpuAccel>(), 16);
        assert_eq!(std::mem::size_of::<EvolveSettings>(), 32);
        assert_eq!(std::mem::size_of::<GpuEvolveState>(), 32);
    }

    #[test]
    fn canonical_transfer_fixture_matches_cuda_layout() {
        let settings = BhSettings {
            info: [1, 2, 3, 4],
            physics: [0.5, 0.25, 1.0, 0.0],
        };
        let body = GpuBody {
            position_mass: [1.0, -2.0, 3.0, 0.0],
            meta: [4, 5, 6, 7],
        };
        let entry = GpuEntry {
            data: [0x0123_4567, 8, 9, 10],
        };
        let cell = GpuCell {
            center_half_mass: [1.0, 2.0, 3.0, 4.0],
            com: [5.0, 6.0, 0.0, 0.0],
            range_depth: [7, 8, 9, 0],
            children: [10, 11, 12, u32::MAX],
        };
        let mut bytes = Vec::new();
        bytes.extend_from_slice(bytemuck::bytes_of(&settings));
        bytes.extend_from_slice(bytemuck::bytes_of(&body));
        bytes.extend_from_slice(bytemuck::bytes_of(&entry));
        bytes.extend_from_slice(bytemuck::bytes_of(&cell));
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(bytes.len(), 144);
        assert_eq!(
            hex,
            "010000000200000003000000040000000000003f0000803e0000803f00000000\
             0000803f000000c0000040400000000004000000050000000600000007000000\
             6745230108000000090000000a0000000000803f000000400000404000008040\
             0000a0400000c040000000000000000007000000080000000900000000000000\
             0a0000000b0000000c000000ffffffff"
                .replace(' ', "")
                .replace('\n', "")
        );
    }

    #[test]
    fn pack_preserves_ranges_children_and_target_positions() {
        let bodies = make_disc(128, 303, 5.0e10, 3.0).unwrap();
        let config = Config::default();
        let tree = build_flat_tree(&bodies, config).unwrap();
        let packed = pack_flat_tree(&bodies, &tree, config).unwrap();

        assert_eq!(packed.settings.info[0] as usize, bodies.len());
        assert_eq!(packed.settings.info[1] as usize, tree.cells.len());
        assert_eq!(packed.settings.info[2], tree.root);
        for (index, body) in packed.bodies.iter().enumerate() {
            assert_eq!(body.meta[0], tree.entry_position_by_body[index]);
        }
        for (source, gpu) in tree.cells.iter().zip(&packed.cells) {
            assert_eq!(gpu.range_depth[0], source.start);
            assert_eq!(gpu.range_depth[1], source.end);
            assert_eq!(gpu.range_depth[2], source.depth as u32);
            assert_eq!(gpu.children, source.children);
        }
        assert!(packed.cells.iter().any(|cell| cell.children != [u32::MAX; 4]));
    }
}
