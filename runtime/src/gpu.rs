// SPDX-License-Identifier: Apache-2.0
use crate::{
    config::{Integrator, Physics, Result, Spin},
    reference::Particle,
};
use bytemuck::{Pod, Zeroable};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::mpsc};
use wgpu::util::DeviceExt;
const BACKENDS: wgpu::Backends = wgpu::Backends::from_bits_retain(
    wgpu::Backends::VULKAN.bits() | wgpu::Backends::METAL.bits() | wgpu::Backends::DX12.bits(),
);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Case {
    pub info: [u32; 4],
    pub p0: [f32; 4],
    pub p1: [f32; 4],
    pub p2: [f32; 4],
}
impl Case {
    pub fn curve(radius: f64, p: &Physics) -> Self {
        Self {
            info: [p.model.id(), 0, 0, 0],
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
                radius as f32,
            ],
        }
    }
    pub fn compact(mass: f64, spin: f64) -> Self {
        let mut p = Self::zeroed();
        p.p0 = [mass as f32, spin as f32, 0.0, 0.0];
        p
    }
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Global {
    info: [u32; 4],
    motion: [f32; 4],
    shape: [f32; 4],
    extra: [f32; 4],
}
impl Global {
    fn spin(s: &Spin, step: u32) -> Self {
        Self {
            info: [s.particles, s.seed, step, s.sample_count()],
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
                if s.integrator == Integrator::Leapfrog {
                    1.0
                } else {
                    0.0
                },
                0.0,
            ],
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
    json!({"name":info.name,"backend":format!("{:?}",info.backend),"device_type":format!("{:?}",info.device_type),
        "driver":info.driver,"driver_info":info.driver_info,"software":software(&info),
        "max_storage_buffer_bytes":limits.max_storage_buffer_binding_size,"max_buffer_bytes":limits.max_buffer_size})
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
pub fn devices() -> Vec<Value> {
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
fn select_adapter(
    infos: &[wgpu::AdapterInfo],
    name: Option<&str>,
    allow_software: bool,
) -> Option<usize> {
    // A listed index takes precedence. Other values, including GPU model numbers,
    // are name substrings. Never switch away from a disallowed software index.
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
pub struct Gpu {
    pub info: Value,
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipelines: HashMap<&'static str, wgpu::ComputePipeline>,
}
pub struct Field {
    global: wgpu::Buffer,
    bind: wgpu::BindGroup,
    output: wgpu::Buffer,
    sample_count: u32,
}
impl Gpu {
    pub fn new(name: Option<&str>, allow_software: bool) -> Result<Self> {
        let mut available_adapters = adapters();
        let infos: Vec<_> = available_adapters.iter().map(|a| a.get_info()).collect();
        let index = select_adapter(&infos, name, allow_software).ok_or("No matching hardware compute adapter. Run `devices`; NVIDIA containers need graphics driver capability. --allow-software permits a software adapter only for validation; --cpu selects the explicit CPU reference.")?;
        let adapter = available_adapters.swap_remove(index);
        let mut info = describe(&adapter);
        info["index"] = json!(index);
        let available = adapter.limits();
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: available.max_storage_buffer_binding_size,
            max_buffer_size: available.max_buffer_size,
            ..wgpu::Limits::default()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("GALAXY native compute"),
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
            label: None,
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let source = format!(
            "{}\n{}",
            include_str!(concat!(env!("OUT_DIR"), "/uff-data.wgsl")),
            include_str!("kernels.wgsl")
        );
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UFF compute kernels"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let mut pipelines = HashMap::new();
        for entry in [
            "initialize",
            "circular",
            "leapfrog",
            "gather",
            "curves",
            "compact",
        ] {
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
            return Err(format!("Requested {size} bytes exceeds this adapter's storage-buffer capacity; reduce particles/evaluations").into());
        }
        Ok(size)
    }
    fn buffers(&self, global: Global, cases: &[Case], count: u32, outputs: u32) -> Result<Field> {
        self.checked_buffer(cases.len() as u64, 64)?;
        let state_bytes = self.checked_buffer(count as u64, 32)?;
        let output_bytes = self.checked_buffer(outputs as u64, 32)?;
        let global = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("job parameters"),
                contents: bytemuck::bytes_of(&global),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let cases = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("UFF cases"),
                contents: bytemuck::cast_slice(cases),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let particles = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("resident particles"),
            size: state_bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let output = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bounded output"),
            size: output_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
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
        Ok(Field {
            global,
            bind,
            output,
            sample_count: outputs,
        })
    }
    fn dispatch(&self, field: &Field, kernel: &str, count: u32, repeats: u32) {
        for start in (0..repeats).step_by(256) {
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
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
    fn read(&self, field: &Field) -> Result<Vec<Particle>> {
        let size = field.sample_count as u64 * 32;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
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
            return Err(
                "GPU produced a non-finite result; reduce dt or check parameter ranges".into(),
            );
        }
        Ok(result)
    }
    pub fn evaluate(&self, kernel: &str, cases: &[Case]) -> Result<Vec<Particle>> {
        if !["curves", "compact"].contains(&kernel)
            || cases.is_empty()
            || cases.len() > crate::config::MAX_CASES
        {
            return Err("Invalid evaluation batch".into());
        }
        let mut global = Global::zeroed();
        global.info[0] = cases.len() as u32;
        let field = self.buffers(global, cases, 1, cases.len() as u32)?;
        self.dispatch(&field, kernel, cases.len() as u32, 1);
        self.read(&field)
    }
    pub fn initialize(&self, s: &Spin) -> Result<Field> {
        s.validate()?;
        let field = self.buffers(
            Global::spin(s, 0),
            &[Case::curve(1.0, &s.physics)],
            s.particles,
            s.sample_count(),
        )?;
        self.dispatch(&field, "initialize", s.particles, 1);
        Ok(field)
    }
    pub fn advance(&self, field: &Field, s: &Spin, steps: u32, total: u32) {
        self.queue.write_buffer(
            &field.global,
            0,
            bytemuck::bytes_of(&Global::spin(s, total)),
        );
        self.dispatch(
            field,
            if s.integrator == Integrator::Circular {
                "circular"
            } else {
                "leapfrog"
            },
            s.particles,
            if s.integrator == Integrator::Circular {
                1
            } else {
                steps
            },
        );
    }
    pub fn sample(&self, field: &Field, s: &Spin) -> Result<Vec<Particle>> {
        self.dispatch(field, "gather", s.sample_count(), 1);
        self.read(field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_selection_supports_model_numbers_and_preserves_index_precedence() {
        let info = |name: &str, device_type| wgpu::AdapterInfo {
            name: name.into(),
            vendor: 0,
            device: 0,
            device_type,
            driver: String::new(),
            driver_info: String::new(),
            backend: wgpu::Backend::Vulkan,
        };
        let infos = [
            info("NVIDIA GeForce RTX 4090", wgpu::DeviceType::DiscreteGpu),
            info("NVIDIA GeForce RTX 5090", wgpu::DeviceType::DiscreteGpu),
            info("llvmpipe", wgpu::DeviceType::Cpu),
        ];
        for (query, expected) in [
            (None, Some(0)),
            (Some("0"), Some(0)),
            (Some("1"), Some(1)),
            (Some("4090"), Some(0)),
            (Some("5090"), Some(1)),
            (Some("nViDiA"), Some(0)),
            (Some("9999"), None),
            (Some("2"), None),
            (Some("LLVMPipe"), None),
        ] {
            assert_eq!(select_adapter(&infos, query, false), expected, "{query:?}");
        }
        assert_eq!(select_adapter(&infos, Some("2"), true), Some(2));
        assert_eq!(select_adapter(&infos, Some("LLVMPipe"), true), Some(2));
    }
}
