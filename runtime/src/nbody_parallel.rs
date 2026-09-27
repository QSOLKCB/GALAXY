// SPDX-License-Identifier: Apache-2.0
//! BH #2D parallel GPU Barnes-Hut tree construction.
//!
//! This module deliberately coexists with the BH #2C serialized GPU builder.
//! The parallel path owns separate sparse level-order buffers and feeds them
//! into the already-frozen Barnes-Hut traversal pipeline.

use crate::{
    config::Result,
    nbody_gpu::{
        BhSettings, EvolvingState, GpuBody, GpuCell, GpuEntry, NbodyGpu, StageTiming, TreeMeta,
        TreeSettings,
    },
};
use bytemuck::{Pod, Zeroable};
use galaxy_nbody::Config;
use std::{sync::mpsc, time::Instant};
use wgpu::util::DeviceExt;

pub const MAX_PARALLEL_TREE_BODIES: u32 = 65_536;
const WORKGROUP_SIZE: u32 = 128;
const RADIX_DIGITS: u32 = 16;
const RADIX_PASSES: u32 = 8;
const MORTON_TREE_DEPTH: u32 = 16;
const TREE_LEVELS: u32 = MORTON_TREE_DEPTH + 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
struct BoundsSettings {
    info: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
struct BoundsRecord {
    values: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
struct RadixSettings {
    info: [u32; 4],
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ParallelTreeBuildTiming {
    pub bounds_seconds: f64,
    pub morton_seconds: f64,
    pub radix_seconds: f64,
    pub positions_seconds: f64,
    pub topology_seconds: f64,
    pub aggregate_seconds: f64,
}

impl ParallelTreeBuildTiming {
    pub fn total_seconds(&self) -> f64 {
        self.bounds_seconds
            + self.morton_seconds
            + self.radix_seconds
            + self.positions_seconds
            + self.topology_seconds
            + self.aggregate_seconds
    }
}

#[derive(Debug)]
pub struct ParallelGpuTree {
    bodies: wgpu::Buffer,
    entries_a: wgpu::Buffer,
    entries_b: wgpu::Buffer,
    cells: wgpu::Buffer,
    meta: wgpu::Buffer,
    bounds_a: wgpu::Buffer,
    bounds_b: wgpu::Buffer,
    histogram: wgpu::Buffer,
    offsets: wgpu::Buffer,
    count: u32,
    blocks: u32,
    cell_capacity: u32,
    bytes: u64,
}

#[derive(Debug)]
pub struct ParallelGpuTreeEvidence {
    pub meta: TreeMeta,
    pub entries: Vec<GpuEntry>,
    pub cells: Vec<GpuCell>,
    pub active_cell_count: usize,
    pub leaf_count: usize,
    pub max_depth: u32,
}

pub struct ParallelTreeRuntime {
    bounds_layout: wgpu::BindGroupLayout,
    bounds_state_pipeline: wgpu::ComputePipeline,
    bounds_reduce_pipeline: wgpu::ComputePipeline,
    bounds_finalize_pipeline: wgpu::ComputePipeline,

    radix_layout: wgpu::BindGroupLayout,
    radix_histogram_pipeline: wgpu::ComputePipeline,
    radix_prefix_pipeline: wgpu::ComputePipeline,
    radix_scatter_pipeline: wgpu::ComputePipeline,

    topology_layout: wgpu::BindGroupLayout,
    topology_morton_pipeline: wgpu::ComputePipeline,
    topology_positions_pipeline: wgpu::ComputePipeline,
    topology_cells_pipeline: wgpu::ComputePipeline,
    topology_links_pipeline: wgpu::ComputePipeline,
    topology_aggregate_pipeline: wgpu::ComputePipeline,
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn checked_storage(gpu: &NbodyGpu, count: usize, stride: usize, name: &str) -> Result<u64> {
    let bytes = (count as u64)
        .checked_mul(stride as u64)
        .ok_or_else(|| format!("{name} buffer size overflow"))?;
    let limits = gpu.device.limits();
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

impl ParallelTreeRuntime {
    pub fn new(gpu: &NbodyGpu) -> Result<Self> {
        let device = &gpu.device;

        let bounds_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("BH #2D parallel bounds layout"),
            entries: &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, false),
                storage_entry(4, false),
            ],
        });
        let bounds_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("BH #2D parallel bounds pipeline layout"),
                bind_group_layouts: &[&bounds_layout],
                push_constant_ranges: &[],
            });
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bounds_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("BH #2D parallel bounds"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("nbody_tree_parallel_bounds.wgsl").into(),
            ),
        });
        let make_bounds = |label: &'static str, entry: &'static str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&bounds_pipeline_layout),
                module: &bounds_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let bounds_state_pipeline =
            make_bounds("bh_parallel_bounds_state", "bh_parallel_bounds_state");
        let bounds_reduce_pipeline =
            make_bounds("bh_parallel_bounds_reduce", "bh_parallel_bounds_reduce");
        let bounds_finalize_pipeline =
            make_bounds("bh_parallel_bounds_finalize", "bh_parallel_bounds_finalize");
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("BH #2D parallel bounds shader validation: {error}").into());
        }

        let radix_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("BH #2D radix layout"),
            entries: &[
                uniform_entry(0),
                storage_entry(1, false),
                storage_entry(2, false),
                storage_entry(3, false),
                storage_entry(4, false),
            ],
        });
        let radix_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("BH #2D radix pipeline layout"),
                bind_group_layouts: &[&radix_layout],
                push_constant_ranges: &[],
            });
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let radix_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("BH #2D stable Morton radix sort"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("nbody_tree_parallel_radix.wgsl").into(),
            ),
        });
        let make_radix = |label: &'static str, entry: &'static str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&radix_pipeline_layout),
                module: &radix_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let radix_histogram_pipeline =
            make_radix("bh_radix_histogram", "bh_radix_histogram");
        let radix_prefix_pipeline = make_radix("bh_radix_prefix", "bh_radix_prefix");
        let radix_scatter_pipeline = make_radix("bh_radix_scatter", "bh_radix_scatter");
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("BH #2D radix shader validation: {error}").into());
        }

        let topology_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("BH #2D topology layout"),
            entries: &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, false),
                storage_entry(3, false),
                storage_entry(4, false),
                storage_entry(5, false),
            ],
        });
        let topology_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("BH #2D topology pipeline layout"),
                bind_group_layouts: &[&topology_layout],
                push_constant_ranges: &[],
            });
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let topology_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("BH #2D parallel topology"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("nbody_tree_parallel_topology.wgsl").into(),
            ),
        });
        let make_topology = |label: &'static str, entry: &'static str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&topology_pipeline_layout),
                module: &topology_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let topology_morton_pipeline =
            make_topology("bh_parallel_tree_morton", "bh_parallel_tree_morton");
        let topology_positions_pipeline =
            make_topology("bh_parallel_tree_positions", "bh_parallel_tree_positions");
        let topology_cells_pipeline =
            make_topology("bh_parallel_tree_cells", "bh_parallel_tree_cells");
        let topology_links_pipeline =
            make_topology("bh_parallel_tree_links", "bh_parallel_tree_links");
        let topology_aggregate_pipeline =
            make_topology("bh_parallel_tree_aggregate", "bh_parallel_tree_aggregate");
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("BH #2D topology shader validation: {error}").into());
        }

        Ok(Self {
            bounds_layout,
            bounds_state_pipeline,
            bounds_reduce_pipeline,
            bounds_finalize_pipeline,
            radix_layout,
            radix_histogram_pipeline,
            radix_prefix_pipeline,
            radix_scatter_pipeline,
            topology_layout,
            topology_morton_pipeline,
            topology_positions_pipeline,
            topology_cells_pipeline,
            topology_links_pipeline,
            topology_aggregate_pipeline,
        })
    }

    fn dispatch(
        &self,
        gpu: &NbodyGpu,
        pipeline: &wgpu::ComputePipeline,
        bind: &wgpu::BindGroup,
        workgroups: u32,
        label: &'static str,
    ) -> f64 {
        let started = Instant::now();
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(label),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }
        gpu.queue.submit(Some(encoder.finish()));
        gpu.device.poll(wgpu::Maintain::Wait);
        started.elapsed().as_secs_f64()
    }

    fn encode_pass(
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        bind: &wgpu::BindGroup,
        workgroups: u32,
        label: &'static str,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(label),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }

    fn submit_wait(
        gpu: &NbodyGpu,
        encoder: wgpu::CommandEncoder,
        started: Instant,
    ) -> f64 {
        gpu.queue.submit(Some(encoder.finish()));
        gpu.device.poll(wgpu::Maintain::Wait);
        started.elapsed().as_secs_f64()
    }

    pub fn create_tree(&self, gpu: &NbodyGpu, evolving: &EvolvingState) -> Result<ParallelGpuTree> {
        if evolving.count < 2 || evolving.count > MAX_PARALLEL_TREE_BODIES {
            return Err(format!(
                "BH #2D parallel GPU tree supports 2..={MAX_PARALLEL_TREE_BODIES} resident bodies"
            )
            .into());
        }

        let count = evolving.count;
        let blocks = count.div_ceil(WORKGROUP_SIZE);
        let cell_capacity = count
            .checked_mul(TREE_LEVELS)
            .ok_or("BH #2D sparse cell capacity overflow")?;

        let body_bytes = checked_storage(
            gpu,
            count as usize,
            std::mem::size_of::<GpuBody>(),
            "BH #2D bodies",
        )?;
        let entry_bytes = checked_storage(
            gpu,
            count as usize,
            std::mem::size_of::<GpuEntry>(),
            "BH #2D Morton entries",
        )?;
        let cell_bytes = checked_storage(
            gpu,
            cell_capacity as usize,
            std::mem::size_of::<GpuCell>(),
            "BH #2D sparse cells",
        )?;
        let bounds_bytes = checked_storage(
            gpu,
            blocks as usize,
            std::mem::size_of::<BoundsRecord>(),
            "BH #2D bounds scratch",
        )?;
        let histogram_words = blocks
            .checked_mul(RADIX_DIGITS)
            .ok_or("BH #2D radix histogram capacity overflow")?;
        let histogram_bytes = checked_storage(
            gpu,
            histogram_words as usize,
            std::mem::size_of::<u32>(),
            "BH #2D radix histogram",
        )?;
        let offsets_bytes = checked_storage(
            gpu,
            histogram_words as usize,
            std::mem::size_of::<u32>(),
            "BH #2D radix offsets",
        )?;
        let meta_bytes = std::mem::size_of::<TreeMeta>() as u64;

        let storage_copy = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
        let bodies = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BH #2D bodies"),
            size: body_bytes,
            usage: storage_copy,
            mapped_at_creation: false,
        });
        let entries_a = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BH #2D Morton entries A"),
            size: entry_bytes,
            usage: storage_copy,
            mapped_at_creation: false,
        });
        let entries_b = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BH #2D Morton entries B"),
            size: entry_bytes,
            usage: storage_copy,
            mapped_at_creation: false,
        });
        let cells = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BH #2D sparse cells"),
            size: cell_bytes,
            usage: storage_copy | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let meta = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BH #2D tree metadata"),
            size: meta_bytes,
            usage: storage_copy,
            mapped_at_creation: false,
        });
        let bounds_a = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BH #2D bounds A"),
            size: bounds_bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let bounds_b = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BH #2D bounds B"),
            size: bounds_bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let histogram = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BH #2D radix histogram"),
            size: histogram_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let offsets = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BH #2D radix offsets"),
            size: offsets_bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        Ok(ParallelGpuTree {
            bodies,
            entries_a,
            entries_b,
            cells,
            meta,
            bounds_a,
            bounds_b,
            histogram,
            offsets,
            count,
            blocks,
            cell_capacity,
            bytes: body_bytes
                + entry_bytes * 2
                + cell_bytes
                + meta_bytes
                + bounds_bytes * 2
                + histogram_bytes
                + offsets_bytes,
        })
    }

    fn bounds_bind<'a>(
        &'a self,
        gpu: &'a NbodyGpu,
        settings: &'a wgpu::Buffer,
        evolving: &'a EvolvingState,
        input: &'a wgpu::Buffer,
        output: &'a wgpu::Buffer,
        tree: &'a ParallelGpuTree,
    ) -> wgpu::BindGroup {
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("BH #2D bounds bindings"),
            layout: &self.bounds_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: settings.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: evolving.state.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: input.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: output.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: tree.meta.as_entire_binding() },
            ],
        })
    }

    fn topology_bind<'a>(
        &'a self,
        gpu: &'a NbodyGpu,
        settings: &'a wgpu::Buffer,
        evolving: &'a EvolvingState,
        tree: &'a ParallelGpuTree,
    ) -> wgpu::BindGroup {
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("BH #2D topology bindings"),
            layout: &self.topology_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: settings.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: evolving.state.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: tree.bodies.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: tree.entries_a.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: tree.cells.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: tree.meta.as_entire_binding() },
            ],
        })
    }

    fn tree_settings(config: Config, count: u32, depth: u32) -> Result<TreeSettings> {
        Ok(TreeSettings {
            info: [
                count,
                config.bucket as u32,
                config.max_depth.min(MORTON_TREE_DEPTH as usize) as u32,
                depth,
            ],
            physics: [
                config.theta as f32,
                config.softening_kpc as f32,
                config.g as f32,
                0.0,
            ],
        })
    }

    pub fn rebuild(
        &self,
        gpu: &NbodyGpu,
        evolving: &EvolvingState,
        tree: &ParallelGpuTree,
        config: Config,
    ) -> Result<ParallelTreeBuildTiming> {
        if tree.count != evolving.count {
            return Err("BH #2D tree count does not match evolving state".into());
        }
        config.validate()?;
        if !config.theta.is_finite()
            || !config.softening_kpc.is_finite()
            || !config.g.is_finite()
        {
            return Err("BH #2D physics parameters must remain finite".into());
        }

        let mut timing = ParallelTreeBuildTiming::default();

        // Bounds reduction is one ordered command buffer: state -> workgroup
        // records, recursive workgroup reduction, then root-bound finalization.
        let bounds_started = Instant::now();
        let mut bounds_encoder =
            gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("BH #2D batched bounds reduction"),
            });
        let first_settings = BoundsSettings {
            info: [tree.count, 0, 0, 0],
        };
        let first_settings = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BH #2D bounds state settings"),
            contents: bytemuck::bytes_of(&first_settings),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let first_bind = self.bounds_bind(
            gpu,
            &first_settings,
            evolving,
            &tree.bounds_b,
            &tree.bounds_a,
            tree,
        );
        Self::encode_pass(
            &mut bounds_encoder,
            &self.bounds_state_pipeline,
            &first_bind,
            tree.blocks,
            "BH #2D state bounds reduction",
        );

        let mut current_count = tree.blocks;
        let mut current_is_a = true;
        let mut bounds_resources: Vec<(wgpu::Buffer, wgpu::BindGroup)> = Vec::new();
        while current_count > 1 {
            let next_count = current_count.div_ceil(WORKGROUP_SIZE);
            let settings_value = BoundsSettings {
                info: [current_count, 0, 0, 0],
            };
            let settings = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("BH #2D bounds reduce settings"),
                contents: bytemuck::bytes_of(&settings_value),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let (input, output) = if current_is_a {
                (&tree.bounds_a, &tree.bounds_b)
            } else {
                (&tree.bounds_b, &tree.bounds_a)
            };
            let bind = self.bounds_bind(gpu, &settings, evolving, input, output, tree);
            Self::encode_pass(
                &mut bounds_encoder,
                &self.bounds_reduce_pipeline,
                &bind,
                next_count,
                "BH #2D recursive bounds reduction",
            );
            bounds_resources.push((settings, bind));
            current_count = next_count;
            current_is_a = !current_is_a;
        }

        let final_settings_value = BoundsSettings { info: [1, 0, 0, 0] };
        let final_settings = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BH #2D bounds finalize settings"),
            contents: bytemuck::bytes_of(&final_settings_value),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let (final_input, final_output) = if current_is_a {
            (&tree.bounds_a, &tree.bounds_b)
        } else {
            (&tree.bounds_b, &tree.bounds_a)
        };
        let final_bind =
            self.bounds_bind(gpu, &final_settings, evolving, final_input, final_output, tree);
        Self::encode_pass(
            &mut bounds_encoder,
            &self.bounds_finalize_pipeline,
            &final_bind,
            1,
            "BH #2D bounds finalize",
        );
        timing.bounds_seconds = Self::submit_wait(gpu, bounds_encoder, bounds_started);
        drop(bounds_resources);

        // Morton generation remains one parallel dispatch and one synchronization.
        let base_settings_value = Self::tree_settings(config, tree.count, 0)?;
        let base_settings = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BH #2D Morton settings"),
            contents: bytemuck::bytes_of(&base_settings_value),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let base_bind = self.topology_bind(gpu, &base_settings, evolving, tree);
        timing.morton_seconds = self.dispatch(
            gpu,
            &self.topology_morton_pipeline,
            &base_bind,
            tree.blocks,
            "BH #2D parallel Morton generation",
        );

        // All eight stable LSD radix passes are ordered inside one command buffer.
        // Command-buffer ordering supplies the inter-pass storage dependency.
        let radix_started = Instant::now();
        let mut radix_encoder =
            gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("BH #2D batched stable radix sort"),
            });
        let mut radix_resources: Vec<(wgpu::Buffer, wgpu::BindGroup)> = Vec::new();
        for pass in 0..RADIX_PASSES {
            radix_encoder.clear_buffer(&tree.histogram, 0, None);
            let settings_value = RadixSettings {
                info: [tree.count, tree.blocks, pass * 4, pass & 1],
            };
            let settings = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("BH #2D radix settings"),
                contents: bytemuck::bytes_of(&settings_value),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("BH #2D radix bindings"),
                layout: &self.radix_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: settings.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: tree.entries_a.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: tree.entries_b.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: tree.histogram.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: tree.offsets.as_entire_binding() },
                ],
            });
            Self::encode_pass(
                &mut radix_encoder,
                &self.radix_histogram_pipeline,
                &bind,
                tree.blocks,
                "BH #2D radix histogram",
            );
            Self::encode_pass(
                &mut radix_encoder,
                &self.radix_prefix_pipeline,
                &bind,
                1,
                "BH #2D radix prefix",
            );
            Self::encode_pass(
                &mut radix_encoder,
                &self.radix_scatter_pipeline,
                &bind,
                tree.blocks,
                "BH #2D radix scatter",
            );
            radix_resources.push((settings, bind));
        }
        timing.radix_seconds = Self::submit_wait(gpu, radix_encoder, radix_started);
        drop(radix_resources);

        let position_settings_value = Self::tree_settings(config, tree.count, 0)?;
        let position_settings = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BH #2D position settings"),
            contents: bytemuck::bytes_of(&position_settings_value),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let position_bind = self.topology_bind(gpu, &position_settings, evolving, tree);
        timing.positions_seconds = self.dispatch(
            gpu,
            &self.topology_positions_pipeline,
            &position_bind,
            tree.blocks,
            "BH #2D target-position assignment",
        );

        // Sparse cell creation and link construction are ordered by depth but
        // submitted as one command buffer, eliminating per-depth CPU waits.
        let max_depth = config.max_depth.min(MORTON_TREE_DEPTH as usize) as u32;
        let topology_started = Instant::now();
        let mut topology_encoder =
            gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("BH #2D batched sparse topology"),
            });
        topology_encoder.clear_buffer(&tree.cells, 0, None);
        let mut topology_resources: Vec<(wgpu::Buffer, wgpu::BindGroup)> = Vec::new();
        for depth in 0..=max_depth {
            let settings_value = Self::tree_settings(config, tree.count, depth)?;
            let settings = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("BH #2D cell level settings"),
                contents: bytemuck::bytes_of(&settings_value),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind = self.topology_bind(gpu, &settings, evolving, tree);
            Self::encode_pass(
                &mut topology_encoder,
                &self.topology_cells_pipeline,
                &bind,
                tree.blocks,
                "BH #2D parallel cell ranges",
            );
            topology_resources.push((settings, bind));

            if depth > 0 {
                let parent_value = Self::tree_settings(config, tree.count, depth - 1)?;
                let parent_settings =
                    gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("BH #2D parent-link settings"),
                        contents: bytemuck::bytes_of(&parent_value),
                        usage: wgpu::BufferUsages::UNIFORM,
                    });
                let parent_bind =
                    self.topology_bind(gpu, &parent_settings, evolving, tree);
                Self::encode_pass(
                    &mut topology_encoder,
                    &self.topology_links_pipeline,
                    &parent_bind,
                    tree.blocks,
                    "BH #2D parallel child links",
                );
                topology_resources.push((parent_settings, parent_bind));
            }
        }
        timing.topology_seconds =
            Self::submit_wait(gpu, topology_encoder, topology_started);
        drop(topology_resources);

        // Reverse-depth aggregate construction is similarly one ordered submit.
        let aggregate_started = Instant::now();
        let mut aggregate_encoder =
            gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("BH #2D batched parallel aggregates"),
            });
        let mut aggregate_resources: Vec<(wgpu::Buffer, wgpu::BindGroup)> = Vec::new();
        for depth in (0..=max_depth).rev() {
            let settings_value = Self::tree_settings(config, tree.count, depth)?;
            let settings = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("BH #2D aggregate-level settings"),
                contents: bytemuck::bytes_of(&settings_value),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind = self.topology_bind(gpu, &settings, evolving, tree);
            Self::encode_pass(
                &mut aggregate_encoder,
                &self.topology_aggregate_pipeline,
                &bind,
                tree.blocks,
                "BH #2D parallel aggregates",
            );
            aggregate_resources.push((settings, bind));
        }
        timing.aggregate_seconds =
            Self::submit_wait(gpu, aggregate_encoder, aggregate_started);
        drop(aggregate_resources);

        Ok(timing)
    }

    pub fn force(
        &self,
        gpu: &NbodyGpu,
        evolving: &EvolvingState,
        tree: &ParallelGpuTree,
        config: Config,
    ) -> Result<StageTiming> {
        if tree.count != evolving.count {
            return Err("BH #2D force/tree count does not match evolving state".into());
        }
        config.validate()?;
        let settings = BhSettings {
            info: [tree.count, tree.cell_capacity, 0, 0],
            physics: [
                config.theta as f32,
                config.softening_kpc as f32,
                config.g as f32,
                0.0,
            ],
        };
        if settings.physics[..3].iter().any(|value| !value.is_finite()) {
            return Err("BH #2D force settings are not representable as finite f32".into());
        }

        let transfer_started = Instant::now();
        let settings = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BH #2D force settings"),
            contents: bytemuck::bytes_of(&settings),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        gpu.device.poll(wgpu::Maintain::Wait);
        let transfer_seconds = transfer_started.elapsed().as_secs_f64();

        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("BH #2D force bindings"),
            layout: &gpu.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: settings.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: tree.bodies.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: tree.entries_a.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: tree.cells.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: evolving.acceleration.as_entire_binding(),
                },
            ],
        });

        let started = Instant::now();
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("BH #2D force traversal"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("BH #2D force traversal"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(tree.blocks, 1, 1);
        }
        gpu.queue.submit(Some(encoder.finish()));
        gpu.device.poll(wgpu::Maintain::Wait);

        Ok(StageTiming {
            transfer_seconds,
            dispatch_seconds: started.elapsed().as_secs_f64(),
            ..StageTiming::default()
        })
    }

    fn read_buffer(
        &self,
        gpu: &NbodyGpu,
        source: &wgpu::Buffer,
        bytes: u64,
        label: &'static str,
    ) -> Result<Vec<u8>> {
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(source, 0, &readback, 0, bytes);
        gpu.queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        gpu.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let view = slice.get_mapped_range();
        let bytes = view.to_vec();
        drop(view);
        readback.unmap();
        Ok(bytes)
    }

    pub fn read_evidence(
        &self,
        gpu: &NbodyGpu,
        tree: &ParallelGpuTree,
    ) -> Result<ParallelGpuTreeEvidence> {
        let meta_bytes = self.read_buffer(
            gpu,
            &tree.meta,
            std::mem::size_of::<TreeMeta>() as u64,
            "BH #2D metadata readback",
        )?;
        let meta = bytemuck::pod_read_unaligned::<TreeMeta>(&meta_bytes);

        let entry_bytes = self.read_buffer(
            gpu,
            &tree.entries_a,
            tree.count as u64 * std::mem::size_of::<GpuEntry>() as u64,
            "BH #2D entry readback",
        )?;
        let entries: Vec<GpuEntry> = entry_bytes
            .chunks_exact(std::mem::size_of::<GpuEntry>())
            .map(bytemuck::pod_read_unaligned::<GpuEntry>)
            .collect();

        let cell_bytes = self.read_buffer(
            gpu,
            &tree.cells,
            tree.cell_capacity as u64 * std::mem::size_of::<GpuCell>() as u64,
            "BH #2D sparse-cell readback",
        )?;
        let all_cells: Vec<GpuCell> = cell_bytes
            .chunks_exact(std::mem::size_of::<GpuCell>())
            .map(bytemuck::pod_read_unaligned::<GpuCell>)
            .collect();
        let cells: Vec<GpuCell> = all_cells
            .into_iter()
            .filter(|cell| cell.range_depth[3] != 0)
            .collect();
        if cells.is_empty() {
            return Err("BH #2D tree evidence contains no active cells".into());
        }

        let mut leaf_count = 0usize;
        let mut max_depth = 0u32;
        for cell in &cells {
            max_depth = max_depth.max(cell.range_depth[2]);
            if cell.children == [u32::MAX; 4] {
                leaf_count += 1;
            }
        }

        Ok(ParallelGpuTreeEvidence {
            meta,
            active_cell_count: cells.len(),
            leaf_count,
            max_depth,
            entries,
            cells,
        })
    }

    pub fn buffer_bytes(&self, tree: &ParallelGpuTree) -> u64 {
        tree.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_tree_control_records_are_frozen() {
        assert_eq!(std::mem::size_of::<BoundsSettings>(), 16);
        assert_eq!(std::mem::size_of::<BoundsRecord>(), 16);
        assert_eq!(std::mem::size_of::<RadixSettings>(), 16);
    }

    #[test]
    fn phase_capacity_keeps_sparse_cells_below_128_mib() {
        let bytes = MAX_PARALLEL_TREE_BODIES as u64
            * TREE_LEVELS as u64
            * std::mem::size_of::<GpuCell>() as u64;
        assert!(bytes < 128 * 1024 * 1024);
    }
}
