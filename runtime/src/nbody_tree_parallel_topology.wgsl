// SPDX-License-Identifier: Apache-2.0
// BH #2D parallel sparse level-order Barnes-Hut topology and aggregate construction.

const NO_CHILD: u32 = 0xffffffffu;

struct TreeSettings {
    info: vec4<u32>,    // body_count, bucket, max_depth, active_depth
    physics: vec4<f32>, // theta, softening, G, reserved
}

struct EvolveState {
    position_mass: vec4<f32>,
    velocity: vec4<f32>,
}

struct BhBody {
    position_mass: vec4<f32>,
    index_data: vec4<u32>,
}

struct BhEntry {
    data: vec4<u32>,
}

struct BhCell {
    center_half_mass: vec4<f32>,
    com: vec4<f32>,
    range_depth: vec4<u32>, // start, end, depth, valid
    children: vec4<u32>,
}

struct TreeMeta {
    data: vec4<u32>,
    bounds: vec4<f32>,
}

@group(0) @binding(0) var<uniform> settings: TreeSettings;
@group(0) @binding(1) var<storage, read> states: array<EvolveState>;
@group(0) @binding(2) var<storage, read_write> bodies: array<BhBody>;
@group(0) @binding(3) var<storage, read> entries: array<BhEntry>;
@group(0) @binding(4) var<storage, read_write> cells: array<BhCell>;
@group(0) @binding(5) var<storage, read_write> tree_meta: TreeMeta;

fn prefix_for(code: u32, depth: u32) -> u32 {
    if depth == 0u {
        return 0u;
    }
    let shift = 32u - 2u * depth;
    return code >> shift;
}

fn set_child(children: vec4<u32>, quadrant: u32, value: u32) -> vec4<u32> {
    var out = children;
    if quadrant == 0u {
        out.x = value;
    } else if quadrant == 1u {
        out.y = value;
    } else if quadrant == 2u {
        out.z = value;
    } else {
        out.w = value;
    }
    return out;
}

fn cell_bounds(code: u32, depth: u32) -> vec3<f32> {
    var cx = tree_meta.bounds.x;
    var cy = tree_meta.bounds.y;
    var half = tree_meta.bounds.z;
    var level = 0u;
    loop {
        if level >= depth {
            break;
        }
        let shift = 30u - 2u * level;
        let quadrant = (code >> shift) & 3u;
        half *= 0.5;
        cx += select(-half, half, (quadrant & 1u) != 0u);
        cy += select(-half, half, (quadrant & 2u) != 0u);
        level += 1u;
    }
    return vec3<f32>(cx, cy, half);
}

fn find_group_end(start: u32, depth: u32, code: u32) -> u32 {
    let count = settings.info.x;
    let prefix = prefix_for(code, depth);
    var end = start + 1u;
    loop {
        if end >= count {
            break;
        }
        if prefix_for(entries[end].data.x, depth) != prefix {
            break;
        }
        end += 1u;
    }
    return end;
}

fn find_parent_range(position: u32, depth: u32, code: u32) -> vec2<u32> {
    let count = settings.info.x;
    let parent_depth = depth - 1u;
    let parent_prefix = prefix_for(code, parent_depth);
    var start = position;
    loop {
        if start == 0u {
            break;
        }
        if prefix_for(entries[start - 1u].data.x, parent_depth) != parent_prefix {
            break;
        }
        start -= 1u;
    }
    var end = position + 1u;
    loop {
        if end >= count {
            break;
        }
        if prefix_for(entries[end].data.x, parent_depth) != parent_prefix {
            break;
        }
        end += 1u;
    }
    return vec2<u32>(start, end);
}

fn empty_cell(bounds: vec3<f32>, start: u32, end: u32, depth: u32) -> BhCell {
    var cell: BhCell;
    cell.center_half_mass = vec4<f32>(bounds, 0.0);
    cell.com = vec4<f32>(0.0);
    cell.range_depth = vec4<u32>(start, end, depth, 1u);
    cell.children = vec4<u32>(NO_CHILD);
    return cell;
}

@compute @workgroup_size(128)
fn bh_parallel_tree_bodies(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= settings.info.x {
        return;
    }
    bodies[id.x].position_mass = states[id.x].position_mass;
    bodies[id.x].index_data = vec4<u32>(0u);
}

@compute @workgroup_size(128)
fn bh_parallel_tree_positions(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= settings.info.x {
        return;
    }
    let body_index = entries[id.x].data.y;
    bodies[body_index].index_data.x = id.x;
}

@compute @workgroup_size(128)
fn bh_parallel_tree_cells(@builtin(global_invocation_id) id: vec3<u32>) {
    let position = id.x;
    let count = settings.info.x;
    let bucket = settings.info.y;
    let max_depth = min(settings.info.z, 16u);
    let depth = settings.info.w;
    if position >= count || depth > max_depth {
        return;
    }

    let code = entries[position].data.x;
    let prefix = prefix_for(code, depth);
    if position > 0u && prefix_for(entries[position - 1u].data.x, depth) == prefix {
        return;
    }

    if depth > 0u {
        let parent = find_parent_range(position, depth, code);
        if parent.y - parent.x <= bucket {
            return;
        }
    }

    let end = find_group_end(position, depth, code);
    let slot = depth * count + position;
    cells[slot] = empty_cell(cell_bounds(code, depth), position, end, depth);
}

@compute @workgroup_size(128)
fn bh_parallel_tree_links(@builtin(global_invocation_id) id: vec3<u32>) {
    let position = id.x;
    let count = settings.info.x;
    let parent_depth = settings.info.w;
    if position >= count || parent_depth >= min(settings.info.z, 16u) {
        return;
    }

    let parent_slot = parent_depth * count + position;
    var parent = cells[parent_slot];
    if parent.range_depth.w == 0u {
        return;
    }

    var children = vec4<u32>(NO_CHILD);
    var cursor = parent.range_depth.x;
    let child_depth = parent_depth + 1u;
    loop {
        if cursor >= parent.range_depth.y {
            break;
        }
        let code = entries[cursor].data.x;
        let shift = 30u - 2u * parent_depth;
        let quadrant = (code >> shift) & 3u;
        let child_slot = child_depth * count + cursor;
        if cells[child_slot].range_depth.w != 0u {
            children = set_child(children, quadrant, child_slot);
        }
        cursor = find_group_end(cursor, child_depth, code);
    }
    parent.children = children;
    cells[parent_slot] = parent;
}

@compute @workgroup_size(128)
fn bh_parallel_tree_aggregate(@builtin(global_invocation_id) id: vec3<u32>) {
    let position = id.x;
    let count = settings.info.x;
    let depth = settings.info.w;
    if position >= count {
        return;
    }

    let slot = depth * count + position;
    var cell = cells[slot];
    if cell.range_depth.w == 0u {
        return;
    }

    let leaf = cell.children.x == NO_CHILD
        && cell.children.y == NO_CHILD
        && cell.children.z == NO_CHILD
        && cell.children.w == NO_CHILD;

    var mass = 0.0;
    var wx = 0.0;
    var wy = 0.0;

    if leaf {
        var p = cell.range_depth.x;
        loop {
            if p >= cell.range_depth.y {
                break;
            }
            let body_index = entries[p].data.y;
            let body = bodies[body_index].position_mass;
            mass += body.z;
            wx += body.z * body.x;
            wy += body.z * body.y;
            p += 1u;
        }
    } else {
        var q = 0u;
        loop {
            if q >= 4u {
                break;
            }
            let child_index = cell.children[q];
            if child_index != NO_CHILD {
                let child = cells[child_index];
                let child_mass = child.center_half_mass.w;
                mass += child_mass;
                wx += child_mass * child.com.x;
                wy += child_mass * child.com.y;
            }
            q += 1u;
        }
    }

    cell.center_half_mass.w = mass;
    cell.com = vec4<f32>(wx / mass, wy / mass, 0.0, 0.0);
    cells[slot] = cell;
}
