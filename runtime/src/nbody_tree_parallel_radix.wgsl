// SPDX-License-Identifier: Apache-2.0
// BH #2D stable block-parallel 4-bit LSD radix ordering of Morton entries.

struct RadixSettings {
    info: vec4<u32>, // count, block_count, shift, direction(0=A->B,1=B->A)
}

struct BhEntry {
    data: vec4<u32>, // Morton code, resident body index, reserved...
}

@group(0) @binding(0) var<uniform> settings: RadixSettings;
@group(0) @binding(1) var<storage, read_write> entries_a: array<BhEntry>;
@group(0) @binding(2) var<storage, read_write> entries_b: array<BhEntry>;
@group(0) @binding(3) var<storage, read_write> histogram: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> offsets: array<u32>;

var<workgroup> local_hist: array<atomic<u32>, 16>;
var<workgroup> totals: array<u32, 16>;
var<workgroup> digits: array<u32, 128>;

fn read_entry(i: u32) -> BhEntry {
    var entry: BhEntry;
    if settings.info.w == 0u {
        entry = entries_a[i];
    } else {
        entry = entries_b[i];
    }
    return entry;
}

@compute @workgroup_size(128)
fn bh_radix_histogram(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
) {
    let lane = local_id.x;
    if lane < 16u {
        atomicStore(&local_hist[lane], 0u);
    }
    workgroupBarrier();

    if id.x < settings.info.x {
        let entry = read_entry(id.x);
        let digit = (entry.data.x >> settings.info.z) & 15u;
        atomicAdd(&local_hist[digit], 1u);
    }
    workgroupBarrier();

    if lane < 16u {
        let global_index = group_id.x * 16u + lane;
        atomicStore(&histogram[global_index], atomicLoad(&local_hist[lane]));
    }
}

@compute @workgroup_size(16)
fn bh_radix_prefix(@builtin(local_invocation_id) local_id: vec3<u32>) {
    let digit = local_id.x;
    let blocks = settings.info.y;

    var total = 0u;
    var block = 0u;
    loop {
        if block >= blocks {
            break;
        }
        total += atomicLoad(&histogram[block * 16u + digit]);
        block += 1u;
    }
    totals[digit] = total;
    workgroupBarrier();

    var digit_base = 0u;
    var d = 0u;
    loop {
        if d >= digit {
            break;
        }
        digit_base += totals[d];
        d += 1u;
    }

    var running = digit_base;
    block = 0u;
    loop {
        if block >= blocks {
            break;
        }
        let index = block * 16u + digit;
        offsets[index] = running;
        running += atomicLoad(&histogram[index]);
        block += 1u;
    }
}

@compute @workgroup_size(128)
fn bh_radix_scatter(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
) {
    let lane = local_id.x;
    var entry: BhEntry;
    var digit = 0xffffffffu;
    if id.x < settings.info.x {
        entry = read_entry(id.x);
        digit = (entry.data.x >> settings.info.z) & 15u;
    }
    digits[lane] = digit;
    workgroupBarrier();

    if id.x >= settings.info.x {
        return;
    }

    var local_rank = 0u;
    var prior = 0u;
    loop {
        if prior >= lane {
            break;
        }
        if digits[prior] == digit {
            local_rank += 1u;
        }
        prior += 1u;
    }

    let destination = offsets[group_id.x * 16u + digit] + local_rank;
    if settings.info.w == 0u {
        entries_b[destination] = entry;
    } else {
        entries_a[destination] = entry;
    }
}
