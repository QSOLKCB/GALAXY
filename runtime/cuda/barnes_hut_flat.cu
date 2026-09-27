// SPDX-License-Identifier: Apache-2.0
// BH #2B1 CUDA traversal counterpart to runtime/src/nbody.wgsl.
// This file freezes the CUDA-side ABI and traversal semantics. CI validates
// layout/source parity without claiming CUDA execution on non-NVIDIA runners.

#include <stdint.h>
#include <math.h>

static constexpr uint32_t NO_CHILD = 0xffffffffu;

struct alignas(16) BhSettings {
    uint32_t info[4];
    float physics[4];
};

struct alignas(16) BhBody {
    float position_mass[4];
    uint32_t meta[4];
};

struct alignas(16) BhEntry {
    uint32_t data[4];
};

struct alignas(16) BhCell {
    float center_half_mass[4];
    float com[4];
    uint32_t range_depth[4];
    uint32_t children[4];
};

static_assert(sizeof(BhSettings) == 32, "BhSettings ABI");
static_assert(sizeof(BhBody) == 32, "BhBody ABI");
static_assert(sizeof(BhEntry) == 16, "BhEntry ABI");
static_assert(sizeof(BhCell) == 64, "BhCell ABI");

struct alignas(16) EvolveSettings {
    uint32_t info[4];
    float motion[4];
};

struct alignas(16) EvolveState {
    float position_mass[4];
    float velocity[4];
};

static_assert(sizeof(EvolveSettings) == 32, "EvolveSettings ABI");
static_assert(sizeof(EvolveState) == 32, "EvolveState ABI");

__device__ inline void add_point_mass(
    float tx, float ty,
    float px, float py, float mass,
    float softening, float g,
    float* ax, float* ay
) {
    const float dx = px - tx;
    const float dy = py - ty;
    const float r2 = dx * dx + dy * dy + softening * softening;
    const float inv_r = rsqrtf(r2);
    const float scale = g * mass * inv_r * inv_r * inv_r;
    *ax += dx * scale;
    *ay += dy * scale;
}

extern "C" __global__ void bh_traverse(
    const BhSettings* settings_ptr,
    const BhBody* bodies,
    const BhEntry* entries,
    const BhCell* cells,
    float4* accelerations
) {
    const BhSettings settings = *settings_ptr;
    const uint32_t target_index = blockIdx.x * blockDim.x + threadIdx.x;
    if (target_index >= settings.info[0]) {
        return;
    }

    const BhBody target_body = bodies[target_index];
    const float tx = target_body.position_mass[0];
    const float ty = target_body.position_mass[1];
    const uint32_t target_position = target_body.meta[0];
    float ax = 0.0f;
    float ay = 0.0f;

    uint32_t stack[64];
    uint32_t stack_len = 1;
    stack[0] = settings.info[2];

    while (stack_len != 0) {
        const uint32_t cell_index = stack[--stack_len];
        const BhCell cell = cells[cell_index];
        const bool leaf =
            cell.children[0] == NO_CHILD &&
            cell.children[1] == NO_CHILD &&
            cell.children[2] == NO_CHILD &&
            cell.children[3] == NO_CHILD;

        if (leaf) {
            for (uint32_t position = cell.range_depth[0];
                 position < cell.range_depth[1];
                 ++position) {
                const uint32_t other_index = entries[position].data[1];
                if (other_index == target_index) {
                    continue;
                }
                const BhBody other = bodies[other_index];
                add_point_mass(
                    tx, ty,
                    other.position_mass[0], other.position_mass[1], other.position_mass[2],
                    settings.physics[1], settings.physics[2],
                    &ax, &ay
                );
            }
            continue;
        }

        const float dx = cell.com[0] - tx;
        const float dy = cell.com[1] - ty;
        const float distance2 = dx * dx + dy * dy;
        const bool contains_target =
            target_position >= cell.range_depth[0] &&
            target_position < cell.range_depth[1];

        if (!contains_target && distance2 > 0.0f) {
            const float distance = sqrtf(distance2);
            if ((2.0f * cell.center_half_mass[2]) / distance < settings.physics[0]) {
                add_point_mass(
                    tx, ty,
                    cell.com[0], cell.com[1], cell.center_half_mass[3],
                    settings.physics[1], settings.physics[2],
                    &ax, &ay
                );
                continue;
            }
        }

        for (int quadrant = 3; quadrant >= 0; --quadrant) {
            const uint32_t child = cell.children[quadrant];
            if (child != NO_CHILD) {
                stack[stack_len++] = child;
            }
        }
    }

    accelerations[target_index] = make_float4(ax, ay, 0.0f, 0.0f);
}


extern "C" __global__ void bh_kick_drift(
    const EvolveSettings* settings_ptr,
    EvolveState* states,
    const float4* accelerations
) {
    const EvolveSettings settings = *settings_ptr;
    const uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= settings.info[0]) {
        return;
    }
    const float dt = settings.motion[0];
    EvolveState state = states[i];
    const float vx_half = state.velocity[0] + 0.5f * dt * accelerations[i].x;
    const float vy_half = state.velocity[1] + 0.5f * dt * accelerations[i].y;
    state.position_mass[0] += dt * vx_half;
    state.position_mass[1] += dt * vy_half;
    state.velocity[0] = vx_half;
    state.velocity[1] = vy_half;
    states[i] = state;
}

extern "C" __global__ void bh_final_kick(
    const EvolveSettings* settings_ptr,
    EvolveState* states,
    const float4* accelerations
) {
    const EvolveSettings settings = *settings_ptr;
    const uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= settings.info[0]) {
        return;
    }
    const float dt = settings.motion[0];
    EvolveState state = states[i];
    state.velocity[0] += 0.5f * dt * accelerations[i].x;
    state.velocity[1] += 0.5f * dt * accelerations[i].y;
    states[i] = state;
}
