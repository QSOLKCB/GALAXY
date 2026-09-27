// SPDX-License-Identifier: Apache-2.0
// BH #2D parallel bounds reduction for GPU-resident Barnes-Hut state.

struct BoundsSettings {
    info: vec4<u32>, // input_count, reserved...
}

struct EvolveState {
    position_mass: vec4<f32>,
    velocity: vec4<f32>,
}

struct BoundsRecord {
    values: vec4<f32>, // min_x, min_y, max_x, max_y
}

struct TreeMeta {
    data: vec4<u32>,
    bounds: vec4<f32>,
}

@group(0) @binding(0) var<uniform> settings: BoundsSettings;
@group(0) @binding(1) var<storage, read> states: array<EvolveState>;
@group(0) @binding(2) var<storage, read> input_bounds: array<BoundsRecord>;
@group(0) @binding(3) var<storage, read_write> output_bounds: array<BoundsRecord>;
@group(0) @binding(4) var<storage, read_write> tree_meta: TreeMeta;

var<workgroup> min_x: array<f32, 128>;
var<workgroup> min_y: array<f32, 128>;
var<workgroup> max_x: array<f32, 128>;
var<workgroup> max_y: array<f32, 128>;

fn reduce_lane(local_index: u32) {
    var stride = 64u;
    loop {
        if stride == 0u {
            break;
        }
        workgroupBarrier();
        if local_index < stride {
            min_x[local_index] = min(min_x[local_index], min_x[local_index + stride]);
            min_y[local_index] = min(min_y[local_index], min_y[local_index + stride]);
            max_x[local_index] = max(max_x[local_index], max_x[local_index + stride]);
            max_y[local_index] = max(max_y[local_index], max_y[local_index + stride]);
        }
        stride >>= 1u;
    }
    workgroupBarrier();
}

@compute @workgroup_size(128)
fn bh_parallel_bounds_state(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
) {
    let i = id.x;
    let lane = local_id.x;
    if i < settings.info.x {
        let p = states[i].position_mass.xy;
        min_x[lane] = p.x;
        min_y[lane] = p.y;
        max_x[lane] = p.x;
        max_y[lane] = p.y;
    } else {
        min_x[lane] = 1.0e38;
        min_y[lane] = 1.0e38;
        max_x[lane] = -1.0e38;
        max_y[lane] = -1.0e38;
    }
    reduce_lane(lane);
    if lane == 0u {
        output_bounds[group_id.x].values =
            vec4<f32>(min_x[0], min_y[0], max_x[0], max_y[0]);
    }
}

@compute @workgroup_size(128)
fn bh_parallel_bounds_reduce(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
) {
    let i = id.x;
    let lane = local_id.x;
    if i < settings.info.x {
        let b = input_bounds[i].values;
        min_x[lane] = b.x;
        min_y[lane] = b.y;
        max_x[lane] = b.z;
        max_y[lane] = b.w;
    } else {
        min_x[lane] = 1.0e38;
        min_y[lane] = 1.0e38;
        max_x[lane] = -1.0e38;
        max_y[lane] = -1.0e38;
    }
    reduce_lane(lane);
    if lane == 0u {
        output_bounds[group_id.x].values =
            vec4<f32>(min_x[0], min_y[0], max_x[0], max_y[0]);
    }
}

@compute @workgroup_size(1)
fn bh_parallel_bounds_finalize(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x != 0u {
        return;
    }
    let b = input_bounds[0].values;
    let span = max(max(b.z - b.x, b.w - b.y), 1.0e-12);
    tree_meta.bounds = vec4<f32>(
        0.5 * (b.x + b.z),
        0.5 * (b.y + b.w),
        0.5 * span + 1.0e-12,
        0.0,
    );
    tree_meta.data = vec4<u32>(0u);
}
