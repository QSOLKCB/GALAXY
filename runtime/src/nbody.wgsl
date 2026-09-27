// SPDX-License-Identifier: Apache-2.0
// BH #2B1: traversal of the CPU-built, Morton-ordered flat Barnes-Hut tree.
// Tree construction remains on the CPU in this phase. All records are explicit
// f32/u32 transfer ABI; no f64 shader feature is required.

const NO_CHILD: u32 = 0xffffffffu;

struct BhSettings {
    info: vec4<u32>,       // body_count, cell_count, root_index, reserved
    physics: vec4<f32>,    // theta, softening_kpc, G, reserved
}

struct BhBody {
    position_mass: vec4<f32>, // x, y, mass, reserved
    index_data: vec4<u32>,    // Morton entry position, reserved...
}

struct BhEntry {
    data: vec4<u32>, // Morton code, body index, reserved...
}

struct BhCell {
    center_half_mass: vec4<f32>, // cx, cy, half, mass
    com: vec4<f32>,              // com_x, com_y, reserved...
    range_depth: vec4<u32>,      // start, end, depth, reserved
    children: vec4<u32>,
}

@group(0) @binding(0) var<uniform> settings: BhSettings;
@group(0) @binding(1) var<storage, read> bodies: array<BhBody>;
@group(0) @binding(2) var<storage, read> entries: array<BhEntry>;
@group(0) @binding(3) var<storage, read> cells: array<BhCell>;
@group(0) @binding(4) var<storage, read_write> accelerations: array<vec4<f32>>;

fn point_mass(target_xy: vec2<f32>, source_xy: vec2<f32>, mass: f32) -> vec2<f32> {
    let delta = source_xy - target_xy;
    let r2 = dot(delta, delta) + settings.physics.y * settings.physics.y;
    let inv_r = inverseSqrt(r2);
    let scale = settings.physics.z * mass * inv_r * inv_r * inv_r;
    return delta * scale;
}

@compute @workgroup_size(128)
fn bh_traverse(@builtin(global_invocation_id) id: vec3<u32>) {
    let target_index = id.x;
    if target_index >= settings.info.x {
        return;
    }

    let target_body = bodies[target_index];
    let target_xy = target_body.position_mass.xy;
    let target_position = target_body.index_data.x;
    var total = vec2<f32>(0.0, 0.0);

    // A quadtree DFS has at most 1 + 3*depth pending nodes. BH #2A caps
    // Morton topology depth at 16, so 64 slots leave deterministic headroom.
    var stack: array<u32, 64>;
    var stack_len = 1u;
    stack[0] = settings.info.z;

    loop {
        if stack_len == 0u {
            break;
        }
        stack_len -= 1u;
        let cell_index = stack[stack_len];
        let cell = cells[cell_index];

        let leaf = cell.children.x == NO_CHILD
            && cell.children.y == NO_CHILD
            && cell.children.z == NO_CHILD
            && cell.children.w == NO_CHILD;

        if leaf {
            var position = cell.range_depth.x;
            loop {
                if position >= cell.range_depth.y {
                    break;
                }
                let entry = entries[position];
                let other_index = entry.data.y;
                if other_index != target_index {
                    let other = bodies[other_index].position_mass;
                    total += point_mass(target_xy, other.xy, other.z);
                }
                position += 1u;
            }
            continue;
        }

        let delta = cell.com.xy - target_xy;
        let distance2 = dot(delta, delta);
        let contains_target =
            target_position >= cell.range_depth.x && target_position < cell.range_depth.y;
        if !contains_target && distance2 > 0.0 {
            let distance = sqrt(distance2);
            if (2.0 * cell.center_half_mass.z) / distance < settings.physics.x {
                total += point_mass(target_xy, cell.com.xy, cell.center_half_mass.w);
                continue;
            }
        }

        // Reverse push preserves the CPU reference's child visitation order 0..3.
        if cell.children.w != NO_CHILD {
            stack[stack_len] = cell.children.w;
            stack_len += 1u;
        }
        if cell.children.z != NO_CHILD {
            stack[stack_len] = cell.children.z;
            stack_len += 1u;
        }
        if cell.children.y != NO_CHILD {
            stack[stack_len] = cell.children.y;
            stack_len += 1u;
        }
        if cell.children.x != NO_CHILD {
            stack[stack_len] = cell.children.x;
            stack_len += 1u;
        }
    }

    accelerations[target_index] = vec4<f32>(total, 0.0, 0.0);
}
