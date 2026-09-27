// SPDX-License-Identifier: Apache-2.0
// BH #2C correctness-first GPU flat-tree construction.
//
// This phase deliberately keeps control-heavy bounds/sort/topology/aggregate
// kernels serialized on one GPU invocation while Morton generation and target
// position assignment are data parallel. The goal is to remove the host tree
// rebuild boundary first; later phases may parallelize these construction stages.

const NO_CHILD: u32 = 0xffffffffu;

struct TreeSettings {
    info: vec4<u32>,       // body_count, padded_count, bucket, max_depth
    physics: vec4<f32>,    // theta, softening, G, reserved
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
    data: vec4<u32>,       // Morton code, body index, reserved...
}

struct BhCell {
    center_half_mass: vec4<f32>, // cx, cy, half, mass
    com: vec4<f32>,              // com_x, com_y, reserved...
    range_depth: vec4<u32>,      // start, end, depth, reserved
    children: vec4<u32>,
}

struct TreeMeta {
    data: vec4<u32>,       // cell_count, leaf_count, max_depth, overflow
    bounds: vec4<f32>,     // root cx, cy, half, reserved
}

@group(0) @binding(0) var<uniform> settings: TreeSettings;
@group(0) @binding(1) var<storage, read> states: array<EvolveState>;
@group(0) @binding(2) var<storage, read_write> bodies: array<BhBody>;
@group(0) @binding(3) var<storage, read_write> entries: array<BhEntry>;
@group(0) @binding(4) var<storage, read_write> cells: array<BhCell>;
@group(0) @binding(5) var<storage, read_write> meta: TreeMeta;

fn spread16(input: u32) -> u32 {
    var value = input & 0x0000ffffu;
    value = (value | (value << 8u)) & 0x00ff00ffu;
    value = (value | (value << 4u)) & 0x0f0f0f0fu;
    value = (value | (value << 2u)) & 0x33333333u;
    value = (value | (value << 1u)) & 0x55555555u;
    return value;
}

fn morton2(x: u32, y: u32) -> u32 {
    return spread16(x) | (spread16(y) << 1u);
}

fn quantize_axis(value: f32, center: f32, half: f32) -> u32 {
    let low = center - half;
    let width = 2.0 * half;
    let scaled = floor((value - low) / width * 65536.0);
    return u32(clamp(scaled, 0.0, 65535.0));
}

fn entry_after(a: BhEntry, b: BhEntry) -> bool {
    return a.data.x > b.data.x || (a.data.x == b.data.x && a.data.y > b.data.y);
}

fn entry_before(a: BhEntry, b: BhEntry) -> bool {
    return a.data.x < b.data.x || (a.data.x == b.data.x && a.data.y < b.data.y);
}

@compute @workgroup_size(1)
fn bh_tree_bounds(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x != 0u {
        return;
    }
    let count = settings.info.x;
    var min_x = states[0].position_mass.x;
    var max_x = min_x;
    var min_y = states[0].position_mass.y;
    var max_y = min_y;
    var i = 1u;
    loop {
        if i >= count {
            break;
        }
        let p = states[i].position_mass.xy;
        min_x = min(min_x, p.x);
        max_x = max(max_x, p.x);
        min_y = min(min_y, p.y);
        max_y = max(max_y, p.y);
        i += 1u;
    }
    let span = max(max(max_x - min_x, max_y - min_y), 1.0e-12);
    meta.bounds = vec4<f32>(
        0.5 * (min_x + max_x),
        0.5 * (min_y + max_y),
        0.5 * span + 1.0e-12,
        0.0,
    );
    meta.data = vec4<u32>(0u, 0u, 0u, 0u);
}

@compute @workgroup_size(128)
fn bh_tree_morton(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    let count = settings.info.x;
    let padded = settings.info.y;
    if i >= padded {
        return;
    }
    if i >= count {
        entries[i].data = vec4<u32>(0xffffffffu, 0xffffffffu, 0u, 0u);
        return;
    }

    let state = states[i];
    let cx = meta.bounds.x;
    let cy = meta.bounds.y;
    let half = meta.bounds.z;
    let qx = quantize_axis(state.position_mass.x, cx, half);
    let qy = quantize_axis(state.position_mass.y, cy, half);
    entries[i].data = vec4<u32>(morton2(qx, qy), i, 0u, 0u);
    bodies[i].position_mass = state.position_mass;
    bodies[i].index_data = vec4<u32>(0u, 0u, 0u, 0u);
}

@compute @workgroup_size(1)
fn bh_tree_sort(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x != 0u {
        return;
    }
    let padded = settings.info.y;
    var k = 2u;
    loop {
        if k > padded {
            break;
        }
        var j = k >> 1u;
        loop {
            if j == 0u {
                break;
            }
            var i = 0u;
            loop {
                if i >= padded {
                    break;
                }
                let ixj = i ^ j;
                if ixj > i && ixj < padded {
                    let ascending = (i & k) == 0u;
                    let a = entries[i];
                    let b = entries[ixj];
                    let swap = select(entry_before(a, b), entry_after(a, b), ascending);
                    if swap {
                        entries[i] = b;
                        entries[ixj] = a;
                    }
                }
                i += 1u;
            }
            j >>= 1u;
        }
        k <<= 1u;
    }
}

@compute @workgroup_size(128)
fn bh_tree_positions(@builtin(global_invocation_id) id: vec3<u32>) {
    let position = id.x;
    if position >= settings.info.x {
        return;
    }
    let body_index = entries[position].data.y;
    bodies[body_index].index_data.x = position;
}

fn child_center(parent: BhCell, quadrant: u32) -> vec3<f32> {
    let h = 0.5 * parent.center_half_mass.z;
    let ox = select(-h, h, (quadrant & 1u) != 0u);
    let oy = select(-h, h, (quadrant & 2u) != 0u);
    return vec3<f32>(
        parent.center_half_mass.x + ox,
        parent.center_half_mass.y + oy,
        h,
    );
}

fn empty_cell(
    cx: f32,
    cy: f32,
    half: f32,
    start: u32,
    end: u32,
    depth: u32,
) -> BhCell {
    var cell: BhCell;
    cell.center_half_mass = vec4<f32>(cx, cy, half, 0.0);
    cell.com = vec4<f32>(0.0);
    cell.range_depth = vec4<u32>(start, end, depth, 0u);
    cell.children = vec4<u32>(NO_CHILD);
    return cell;
}

@compute @workgroup_size(1)
fn bh_tree_topology(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x != 0u {
        return;
    }

    let count = settings.info.x;
    let bucket = settings.info.z;
    let max_depth = min(settings.info.w, 16u);
    let capacity = arrayLength(&cells);

    cells[0] = empty_cell(
        meta.bounds.x,
        meta.bounds.y,
        meta.bounds.z,
        0u,
        count,
        0u,
    );

    var next_free = 1u;
    var level_start = 0u;
    var level_end = 1u;
    var leaf_count = 0u;
    var max_seen = 0u;

    loop {
        if level_start >= level_end {
            break;
        }

        var parent_index = level_start;
        loop {
            if parent_index >= level_end {
                break;
            }

            var parent = cells[parent_index];
            let start = parent.range_depth.x;
            let end = parent.range_depth.y;
            let depth = parent.range_depth.z;
            max_seen = max(max_seen, depth);

            if end - start <= bucket || depth >= max_depth {
                leaf_count += 1u;
                parent_index += 1u;
                continue;
            }

            let shift = 30u - 2u * depth;
            var cursor = start;
            loop {
                if cursor >= end {
                    break;
                }
                let quadrant = (entries[cursor].data.x >> shift) & 3u;
                let group_start = cursor;
                cursor += 1u;
                loop {
                    if cursor >= end {
                        break;
                    }
                    let next_quadrant = (entries[cursor].data.x >> shift) & 3u;
                    if next_quadrant != quadrant {
                        break;
                    }
                    cursor += 1u;
                }

                if next_free >= capacity {
                    meta.data = vec4<u32>(next_free, leaf_count, max_seen, 1u);
                    return;
                }

                let center = child_center(parent, quadrant);
                cells[next_free] = empty_cell(
                    center.x,
                    center.y,
                    center.z,
                    group_start,
                    cursor,
                    depth + 1u,
                );
                parent.children[quadrant] = next_free;
                next_free += 1u;
            }
            cells[parent_index] = parent;
            parent_index += 1u;
        }

        if next_free == level_end {
            break;
        }
        level_start = level_end;
        level_end = next_free;
    }

    meta.data = vec4<u32>(next_free, leaf_count, max_seen, 0u);
}

@compute @workgroup_size(1)
fn bh_tree_aggregate(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x != 0u {
        return;
    }
    if meta.data.w != 0u {
        return;
    }

    var remaining = meta.data.x;
    loop {
        if remaining == 0u {
            break;
        }
        remaining -= 1u;
        var cell = cells[remaining];
        let leaf = cell.children.x == NO_CHILD
            && cell.children.y == NO_CHILD
            && cell.children.z == NO_CHILD
            && cell.children.w == NO_CHILD;

        var mass = 0.0;
        var wx = 0.0;
        var wy = 0.0;

        if leaf {
            var position = cell.range_depth.x;
            loop {
                if position >= cell.range_depth.y {
                    break;
                }
                let body_index = entries[position].data.y;
                let body = bodies[body_index].position_mass;
                mass += body.z;
                wx += body.z * body.x;
                wy += body.z * body.y;
                position += 1u;
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
        cells[remaining] = cell;
    }
}
