// SPDX-License-Identifier: Apache-2.0
// BH #2B2: GPU kick/drift/final-kick stages for persistent resident state.
// Force evaluation remains in nbody.wgsl; this shader consumes the force buffer.

struct EvolveSettings {
    info: vec4<u32>,    // body_count, reserved...
    motion: vec4<f32>,  // dt_myr, reserved...
}

struct EvolveState {
    position_mass: vec4<f32>, // x, y, mass, reserved
    velocity: vec4<f32>,      // vx, vy, reserved...
}

@group(0) @binding(0) var<uniform> settings: EvolveSettings;
@group(0) @binding(1) var<storage, read_write> states: array<EvolveState>;
@group(0) @binding(2) var<storage, read> accelerations: array<vec4<f32>>;

@compute @workgroup_size(128)
fn bh_kick_drift(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= settings.info.x {
        return;
    }
    let dt = settings.motion.x;
    var state = states[i];
    let half_velocity = state.velocity.xy + 0.5 * dt * accelerations[i].xy;
    state.position_mass.x = state.position_mass.x + dt * half_velocity.x;
    state.position_mass.y = state.position_mass.y + dt * half_velocity.y;
    state.velocity.x = half_velocity.x;
    state.velocity.y = half_velocity.y;
    states[i] = state;
}

@compute @workgroup_size(128)
fn bh_final_kick(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= settings.info.x {
        return;
    }
    let dt = settings.motion.x;
    var state = states[i];
    state.velocity.x = state.velocity.x + 0.5 * dt * accelerations[i].x;
    state.velocity.y = state.velocity.y + 0.5 * dt * accelerations[i].y;
    states[i] = state;
}
