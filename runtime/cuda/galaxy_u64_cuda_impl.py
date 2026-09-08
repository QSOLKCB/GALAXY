#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""CUDA backend for GALAXY's memory-bounded u64 tiled spin runtime.

This backend mirrors runtime/src/bin/u64_kernels.wgsl using CuPy RawKernel.
It requires only an NVIDIA driver plus CuPy's CUDA toolkit wheels on the target
machine; Rust, Cargo, Vulkan, and a Vulkan ICD are not required at runtime.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import struct
import sys
import time
import zlib
from pathlib import Path
from typing import Any

MAX_TILE_PARTICLES = 8_388_608
MAX_SNAPSHOTS = 256
MAX_LEAPFROG_STEPS_PER_LAUNCH = 256
MAX_LEAPFROG_PARTICLE_UPDATES_PER_LAUNCH = MAX_TILE_PARTICLES
KPC_TO_M = 3.085677581491367e19
G = 6.67430e-11 * 1.98847e30 / (KPC_TO_M * 1e6)
KMS_TO_KPC_MYR = 31557600.0 * 1e6 * 1000.0 / KPC_TO_M
TAU = math.tau

ROOT = Path(__file__).resolve().parents[2]
DEMO_PATH = ROOT / "data" / "uff" / "DEMO_GALAXY.csv"
PROVENANCE_PATH = ROOT / "data" / "uff" / "provenance.json"
BOOTSTRAP_PATH = ROOT / "scripts" / "bootstrap-cuda.sh"
RUNNER_PATH = ROOT / "scripts" / "run-u64.sh"
RUNTIME_SOURCE_PATHS = (
    Path(__file__).resolve(),
    DEMO_PATH,
    PROVENANCE_PATH,
    BOOTSTRAP_PATH,
    RUNNER_PATH,
)

MODEL_IDS = {
    "baryons": 1,
    "nfw": 2,
    "burkert": 3,
    "mond-rar": 4,
    "uff-empirical": 5,
}

PHYSICS_DEFAULTS = {
    "model": "uff-empirical",
    "disk_ml": 0.5,
    "bulge_ml": 0.7,
    "black_hole_million": 0.0,
    "uff_v_inf": 120.0,
    "uff_core": 3.0,
    "uff_beta": 0.0,
    "halo_log_mass": 11.5,
    "halo_concentration": 10.0,
    "burkert_log_density": 7.5,
    "burkert_core": 5.0,
    "mond_a0": 1.2,
}

TASK_DEFAULTS = {
    "physics": PHYSICS_DEFAULTS,
    "integrator": "circular",
    "logical_particles": 262_144,
    "tile_particles": MAX_TILE_PARTICLES,
    "seed": 303,
    "steps": 400,
    "dt_myr": 0.25,
    "snapshot_every": 100,
    "snapshot_limit": 65_536,
    "arms": 2,
    "pitch_deg": 22.0,
    "scatter": 0.45,
    "bulge_fraction": 0.18,
    "thickness_kpc": 0.3,
    "direction": 1,
    "radial_kick_kms": 0.0,
    "softening_kpc": 0.02,
    "image_size": 1024,
    "extent_kpc": 16.0,
    "inclination_deg": 38.0,
}

CUDA_SOURCE_TEMPLATE = r"""
#include <stdint.h>
#include <math.h>

#define GALAXY_PI 3.14159265358979323846f
#define GALAXY_TAU 6.28318530717958647692f
#define GALAXY_G 4.301047329314801e-6f
#define GALAXY_KMS_TO_KPC_MYR 0.001022712165045695f

__GALAXY_DEMO_CONSTANTS__

__device__ inline void components(float r, float* gas, float* disk, float* bulge) {
    if (r <= DEMO_R[0]) {
        *gas = DEMO_V[0]; *disk = DEMO_V[1]; *bulge = DEMO_V[2]; return;
    }
    for (int i = 1; i < GALAXY_DEMO_N; ++i) {
        if (r <= DEMO_R[i]) {
            float t = (r - DEMO_R[i-1]) / (DEMO_R[i] - DEMO_R[i-1]);
            int a = 3*(i-1), b = 3*i;
            *gas = DEMO_V[a] + t*(DEMO_V[b] - DEMO_V[a]);
            *disk = DEMO_V[a+1] + t*(DEMO_V[b+1] - DEMO_V[a+1]);
            *bulge = DEMO_V[a+2] + t*(DEMO_V[b+2] - DEMO_V[a+2]);
            return;
        }
    }
    int last = 3*(GALAXY_DEMO_N-1);
    *gas = DEMO_V[last]; *disk = DEMO_V[last+1]; *bulge = DEMO_V[last+2];
}

__device__ inline float nfw_shape(float x) {
    if (x < 0.1f) {
        return x*x*(0.5f+x*(-2.0f/3.0f+x*(0.75f+x*(-0.8f+x*(5.0f/6.0f-x*6.0f/7.0f)))));
    }
    return logf(1.0f+x)-x/(1.0f+x);
}

__device__ inline float velocity(
    float r,
    unsigned int model,
    float disk_ml, float bulge_ml, float black_hole_million,
    float uff_v_inf, float uff_core, float uff_beta,
    float halo_log_mass, float halo_concentration,
    float burkert_log_density, float burkert_core, float mond_a0
) {
    float gas, disk, bulge;
    components(r, &gas, &disk, &bulge);
    float total = fmaxf(0.0f, gas*fabsf(gas) + disk_ml*disk*disk + bulge_ml*bulge*bulge)
        + GALAXY_G*black_hole_million*1e6f/r;
    switch (model) {
        case 2u: {
            float mass = powf(10.0f, halo_log_mass);
            float c = halo_concentration;
            float rho = 3.0f*0.07f*0.07f/(8.0f*GALAXY_PI*GALAXY_G);
            float r200 = powf(3.0f*mass/(4.0f*GALAXY_PI*200.0f*rho), 1.0f/3.0f);
            total += GALAXY_G*mass*nfw_shape(c*r/r200)/nfw_shape(c)/r;
            break;
        }
        case 3u: {
            float x = r/burkert_core;
            float shape;
            if (x < 0.001f) {
                shape = 4.0f/3.0f*x*x*x;
            } else if (x < 0.5f) {
                float q = x*x*x*x;
                shape = x*x*x*(4.0f/3.0f-x+q*(4.0f/7.0f-0.5f*x+q*(4.0f/11.0f-x/3.0f+q*(4.0f/15.0f-0.25f*x+q*(4.0f/19.0f-0.2f*x)))));
            } else {
                shape = logf((1.0f+x)*(1.0f+x)*(1.0f+x*x))-2.0f*atanf(x);
            }
            total += GALAXY_G*GALAXY_PI*powf(10.0f,burkert_log_density)*powf(burkert_core,3.0f)*shape/r;
            break;
        }
        case 4u: {
            if (total > 0.0f) {
                float root = sqrtf(total*1e6f/(r*3.085677581491367e19f)/(mond_a0*1e-10f));
                float denominator;
                if (root < 0.1f) {
                    denominator = root*(1.0f+root*(-0.5f+root*(1.0f/6.0f+root*(-1.0f/24.0f+root/120.0f))));
                } else {
                    denominator = 1.0f-expf(-root);
                }
                total /= denominator;
            }
            break;
        }
        case 5u: {
            float x = r/uff_core;
            float q = x*x;
            float shape;
            if (x < 0.15f) {
                shape = q*(1.0f/3.0f+q*(-0.2f+q*(1.0f/7.0f+q*(-1.0f/9.0f+q/11.0f))));
            } else {
                shape = fmaxf(0.0f,1.0f-atanf(x)/x);
            }
            total += uff_v_inf*uff_v_inf*shape*expf(2.0f*uff_beta*x/(1.0f+x));
            break;
        }
        default: break;
    }
    return sqrtf(total);
}

__device__ inline uint32_t hash32(uint32_t x) {
    x ^= x >> 16u;
    x *= 0x7feb352du;
    x ^= x >> 15u;
    x *= 0x846ca68bu;
    return x ^ (x >> 16u);
}

__device__ inline void global_id(uint32_t local_index, uint32_t base_lo, uint32_t base_hi, uint32_t* lo, uint32_t* hi) {
    uint32_t low = base_lo + local_index;
    uint32_t carry = low < base_lo ? 1u : 0u;
    *lo = low;
    *hi = base_hi + carry;
}

__device__ inline uint32_t address_word(uint32_t local_index, uint32_t lane, uint32_t seed, uint32_t base_lo, uint32_t base_hi) {
    uint32_t lo, hi;
    global_id(local_index, base_lo, base_hi, &lo, &hi);
    uint32_t lane_key = (lane + 1u) * 0x9e3779b9u;
    uint32_t low_mix = hash32(lo ^ seed ^ lane_key ^ hash32(hi ^ 0xa511e9b3u));
    return hash32(low_mix ^ hi*0x9e3779b9u ^ 0x85ebca6bu);
}

__device__ inline float random_global(uint32_t local_index, uint32_t lane, uint32_t seed, uint32_t base_lo, uint32_t base_hi) {
    return (float)(address_word(local_index, lane, seed, base_lo, base_hi) >> 8u) / 16777216.0f;
}

extern "C" __global__ void initialize(
    float* particles, uint32_t count, uint32_t seed, uint32_t base_lo, uint32_t base_hi,
    uint32_t model,
    float disk_ml, float bulge_ml, float black_hole_million,
    float uff_v_inf, float uff_core, float uff_beta,
    float halo_log_mass, float halo_concentration,
    float burkert_log_density, float burkert_core, float mond_a0,
    uint32_t integrator, float softening_kpc,
    float arms, float pitch_rad, float scatter, float bulge_fraction,
    float thickness_kpc, float direction, float radial_kick_kms
) {
    uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= count) return;

    float u = random_global(i,0u,seed,base_lo,base_hi);
    float v = random_global(i,1u,seed,base_lo,base_hi);
    float w = random_global(i,2u,seed,base_lo,base_hi);
    float kind = random_global(i,4u,seed,base_lo,base_hi);
    bool bulge = kind < bulge_fraction;
    bool halo = kind >= 0.97f;

    float r = 0.06f + 0.92f*powf(u,1.4f);
    if (bulge) r = 0.015f + 0.27f*powf(u,1.8f);
    else if (halo) r = 0.25f + 0.85f*u;

    float theta = GALAXY_TAU*v;
    if (!bulge && !halo) {
        theta = floorf(v*arms)*GALAXY_TAU/arms
            + logf(r/0.1f)/tanf(pitch_rad)
            + (w-0.5f)*scatter;
    }

    float height = thickness_kpc*(0.4f+r);
    if (bulge) height = 2.28f*(1.0f-r/0.3f);
    else if (halo) height = 4.8f;
    float z = (random_global(i,3u,seed,base_lo,base_hi)-0.5f)*2.0f*height;

    r *= 12.0f;
    float speed = velocity(r,model,disk_ml,bulge_ml,black_hole_million,uff_v_inf,uff_core,uff_beta,
        halo_log_mass,halo_concentration,burkert_log_density,burkert_core,mond_a0)*GALAXY_KMS_TO_KPC_MYR;
    if (integrator != 0u) {
        float soft_r = sqrtf(r*r + softening_kpc*softening_kpc);
        speed = velocity(soft_r,model,disk_ml,bulge_ml,black_hole_million,uff_v_inf,uff_core,uff_beta,
            halo_log_mass,halo_concentration,burkert_log_density,burkert_core,mond_a0)
            * GALAXY_KMS_TO_KPC_MYR*r/soft_r;
    }
    float omega = direction*speed/r;
    float kick = radial_kick_kms*GALAXY_KMS_TO_KPC_MYR;
    float s = sinf(theta), c = cosf(theta);

    uint64_t o = (uint64_t)i*8ull;
    particles[o+0] = r;
    particles[o+1] = theta;
    particles[o+2] = z;
    particles[o+3] = omega;
    particles[o+4] = r*c;
    particles[o+5] = r*s;
    particles[o+6] = -omega*r*s + kick*c;
    particles[o+7] = omega*r*c + kick*s;
}

extern "C" __global__ void circular(
    float* particles, uint32_t count, float phase0, float phase1, float phase2, float phase3
) {
    uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= count) return;
    uint64_t o = (uint64_t)i*8ull;
    float r = particles[o+0];
    float theta0 = particles[o+1];
    float omega = particles[o+3];
    float rate_high = __uint_as_float(__float_as_uint(omega) & 0xfffff000u);
    float rate_low = omega-rate_high;
    float turns = theta0/GALAXY_TAU;
    turns -= floorf(turns);
    float phases[4] = { phase0, phase1, phase2, phase3 };
    #pragma unroll
    for (int j = 0; j < 4; ++j) {
        float a = rate_high*phases[j]; a -= floorf(a);
        turns += a; turns -= floorf(turns);
        float b = rate_low*phases[j]; b -= floorf(b);
        turns += b; turns -= floorf(turns);
    }
    float theta = (turns - (turns > 0.5f ? 1.0f : 0.0f))*GALAXY_TAU;
    float s = sinf(theta), c = cosf(theta);
    particles[o+4] = r*c;
    particles[o+5] = r*s;
    particles[o+6] = -omega*r*s;
    particles[o+7] = omega*r*c;
}

__device__ inline void acceleration(
    float x, float y, float softening_kpc,
    uint32_t model,
    float disk_ml, float bulge_ml, float black_hole_million,
    float uff_v_inf, float uff_core, float uff_beta,
    float halo_log_mass, float halo_concentration,
    float burkert_log_density, float burkert_core, float mond_a0,
    float* ax, float* ay
) {
    float r = sqrtf(x*x+y*y+softening_kpc*softening_kpc);
    float speed = velocity(r,model,disk_ml,bulge_ml,black_hole_million,uff_v_inf,uff_core,uff_beta,
        halo_log_mass,halo_concentration,burkert_log_density,burkert_core,mond_a0)*GALAXY_KMS_TO_KPC_MYR;
    float factor = -(speed*speed/(r*r));
    *ax = factor*x;
    *ay = factor*y;
}

extern "C" __global__ void leapfrog(
    float* particles, uint32_t count, uint32_t repeats, float dt, float softening_kpc,
    uint32_t model,
    float disk_ml, float bulge_ml, float black_hole_million,
    float uff_v_inf, float uff_core, float uff_beta,
    float halo_log_mass, float halo_concentration,
    float burkert_log_density, float burkert_core, float mond_a0
) {
    uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= count) return;
    uint64_t o = (uint64_t)i*8ull;
    float x = particles[o+4], y = particles[o+5];
    float vx = particles[o+6], vy = particles[o+7];

    for (uint32_t step = 0; step < repeats; ++step) {
        float ax0, ay0;
        acceleration(x,y,softening_kpc,model,disk_ml,bulge_ml,black_hole_million,uff_v_inf,uff_core,uff_beta,
            halo_log_mass,halo_concentration,burkert_log_density,burkert_core,mond_a0,&ax0,&ay0);
        float hvx = vx + 0.5f*dt*ax0;
        float hvy = vy + 0.5f*dt*ay0;
        x += dt*hvx;
        y += dt*hvy;
        float ax1, ay1;
        acceleration(x,y,softening_kpc,model,disk_ml,bulge_ml,black_hole_million,uff_v_inf,uff_core,uff_beta,
            halo_log_mass,halo_concentration,burkert_log_density,burkert_core,mond_a0,&ax1,&ay1);
        vx = hvx + 0.5f*dt*ax1;
        vy = hvy + 0.5f*dt*ay1;
    }

    particles[o+4] = x;
    particles[o+5] = y;
    particles[o+6] = vx;
    particles[o+7] = vy;
}

extern "C" __global__ void gather(
    const float* particles, const uint32_t* indices, float* output, uint32_t sample_count
) {
    uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= sample_count) return;
    uint32_t local = indices[i];
    uint64_t src = (uint64_t)local*8ull;
    uint64_t dst = (uint64_t)i*8ull;
    #pragma unroll
    for (int j=0; j<8; ++j) output[dst+j] = particles[src+j];
}
"""


def _f32(value: float) -> float:
    return struct.unpack("<f", struct.pack("<f", float(value)))[0]


def _u32(value: int) -> int:
    return value & 0xFFFFFFFF


def hash32(value: int) -> int:
    x = _u32(value)
    x ^= x >> 16
    x = _u32(x * 0x7FEB352D)
    x ^= x >> 15
    x = _u32(x * 0x846CA68B)
    return _u32(x ^ (x >> 16))


def address_word(index: int, seed: int, lane: int) -> int:
    lo = index & 0xFFFFFFFF
    hi = (index >> 32) & 0xFFFFFFFF
    lane_key = _u32((lane + 1) * 0x9E3779B9)
    low_mix = hash32(lo ^ seed ^ lane_key ^ hash32(hi ^ 0xA511E9B3))
    return hash32(low_mix ^ _u32(hi * 0x9E3779B9) ^ 0x85EBCA6B)


def random_global(index: int, seed: int, lane: int) -> float:
    return (address_word(index, seed, lane) >> 8) / 16_777_216.0


def address_fingerprint(index: int, seed: int) -> str:
    return "".join(f"{address_word(index, seed, lane):08x}" for lane in range(5))


def _deep_defaults(task: dict[str, Any]) -> dict[str, Any]:
    unknown = set(task) - set(TASK_DEFAULTS)
    if unknown:
        raise ValueError(f"Unknown u64 task fields: {', '.join(sorted(unknown))}")
    resolved: dict[str, Any] = {}
    for key, default in TASK_DEFAULTS.items():
        if key == "physics":
            raw = task.get("physics", {})
            if not isinstance(raw, dict):
                raise ValueError("physics must be an object")
            unknown_physics = set(raw) - set(PHYSICS_DEFAULTS)
            if unknown_physics:
                raise ValueError(f"Unknown physics fields: {', '.join(sorted(unknown_physics))}")
            resolved[key] = {**PHYSICS_DEFAULTS, **raw}
        else:
            resolved[key] = task.get(key, default)
    return resolved


def _finite_number(value: Any, name: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{name} must be numeric")
    result = float(value)
    if not math.isfinite(result):
        raise ValueError(f"{name} must be finite")
    return result


def _bounded(value: Any, minimum: float, maximum: float, name: str) -> float:
    result = _finite_number(value, name)
    if result < minimum or result > maximum:
        raise ValueError(f"{name} must be in [{minimum}, {maximum}]")
    return result


def validate_job(job: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(job, dict):
        raise ValueError("Job must be a JSON object")
    unknown = set(job) - {"schema_version", "task"}
    if unknown:
        raise ValueError(f"Unknown job fields: {', '.join(sorted(unknown))}")
    schema_version = job.get("schema_version")
    if isinstance(schema_version, bool) or not isinstance(schema_version, int):
        raise ValueError("schema_version must be an integer")
    if schema_version != 1:
        raise ValueError("Unsupported galaxy-u64 schema_version; expected 1")
    raw_task = job.get("task")
    if not isinstance(raw_task, dict):
        raise ValueError("task must be an object")
    task = _deep_defaults(raw_task)
    physics = task["physics"]

    model = physics["model"]
    if model not in MODEL_IDS:
        raise ValueError(f"physics.model must be one of: {', '.join(MODEL_IDS)}")
    physics["disk_ml"] = _bounded(physics["disk_ml"], 0.0, 1.5, "physics.disk_ml")
    physics["bulge_ml"] = _bounded(physics["bulge_ml"], 0.0, 2.0, "physics.bulge_ml")
    physics["black_hole_million"] = _bounded(
        physics["black_hole_million"], 0.0, 1000.0, "physics.black_hole_million"
    )
    physics["uff_v_inf"] = _bounded(physics["uff_v_inf"], 0.0, 500.0, "physics.uff_v_inf")
    physics["uff_core"] = _bounded(physics["uff_core"], 0.02, 100.0, "physics.uff_core")
    physics["uff_beta"] = _bounded(physics["uff_beta"], -1.0, 1.0, "physics.uff_beta")
    physics["halo_log_mass"] = _bounded(
        physics["halo_log_mass"], 8.0, 14.5, "physics.halo_log_mass"
    )
    physics["halo_concentration"] = _bounded(
        physics["halo_concentration"], 1.0, 40.0, "physics.halo_concentration"
    )
    physics["burkert_log_density"] = _bounded(
        physics["burkert_log_density"], 4.0, 11.0, "physics.burkert_log_density"
    )
    physics["burkert_core"] = _bounded(
        physics["burkert_core"], 0.05, 100.0, "physics.burkert_core"
    )
    physics["mond_a0"] = _bounded(physics["mond_a0"], 0.03, 6.3, "physics.mond_a0")

    integrator = task["integrator"]
    if integrator not in {"circular", "leapfrog"}:
        raise ValueError("integrator must be circular or leapfrog")
    for name in (
        "logical_particles", "tile_particles", "seed", "steps", "snapshot_every",
        "snapshot_limit", "arms", "image_size", "direction",
    ):
        if isinstance(task[name], bool) or not isinstance(task[name], int):
            raise ValueError(f"{name} must be an integer")
    if task["logical_particles"] <= 0 or task["logical_particles"] > 0xFFFFFFFFFFFFFFFF:
        raise ValueError("logical_particles must be in 1..=2^64-1")
    if not (1 <= task["tile_particles"] <= MAX_TILE_PARTICLES):
        raise ValueError(f"tile_particles must be in 1..={MAX_TILE_PARTICLES}")
    if not (0 <= task["seed"] <= 0xFFFFFFFF):
        raise ValueError("seed must be in 0..=2^32-1")
    if not (1 <= task["steps"] <= 100_000):
        raise ValueError("steps must be in 1..=100000")
    if not (0 <= task["snapshot_every"] <= 0xFFFFFFFF):
        raise ValueError("snapshot_every must be in 0..=2^32-1")
    if not (1 <= task["snapshot_limit"] <= 65_536):
        raise ValueError("snapshot_limit must be in 1..=65536")
    if not (1 <= task["arms"] <= 8):
        raise ValueError("arms must be in 1..=8")
    if task["direction"] not in (1, -1):
        raise ValueError("direction must be 1 or -1")
    if not (128 <= task["image_size"] <= 2048):
        raise ValueError("image_size must be in 128..=2048")

    task["dt_myr"] = _bounded(task["dt_myr"], 0.0001, 2.0, "dt_myr")
    task["pitch_deg"] = _bounded(task["pitch_deg"], 10.0, 40.0, "pitch_deg")
    task["scatter"] = _bounded(task["scatter"], 0.0, 2.5, "scatter")
    task["bulge_fraction"] = _bounded(task["bulge_fraction"], 0.0, 0.5, "bulge_fraction")
    task["thickness_kpc"] = _bounded(task["thickness_kpc"], 0.0, 2.0, "thickness_kpc")
    task["radial_kick_kms"] = _bounded(
        task["radial_kick_kms"], -100.0, 100.0, "radial_kick_kms"
    )
    task["softening_kpc"] = _bounded(task["softening_kpc"], 0.001, 0.2, "softening_kpc")
    task["extent_kpc"] = _bounded(task["extent_kpc"], 1.0, 100.0, "extent_kpc")
    task["inclination_deg"] = _bounded(task["inclination_deg"], 0.0, 90.0, "inclination_deg")
    if integrator == "circular" and task["radial_kick_kms"] != 0.0:
        raise ValueError("radial_kick_kms requires the leapfrog integrator")

    snapshots = snapshot_count(task)
    if snapshots > MAX_SNAPSHOTS:
        raise ValueError("At most 256 snapshots per job; increase snapshot_every")
    updates = particle_updates(task)
    if updates > 0xFFFFFFFFFFFFFFFF:
        raise ValueError("particle update count exceeds the u64 receipt contract")
    if task["logical_particles"] + task["tile_particles"] - 1 > 0xFFFFFFFFFFFFFFFF:
        raise ValueError("logical particle/tile range overflows u64")
    return {"schema_version": 1, "task": task}


def load_job(path: Path) -> dict[str, Any]:
    if path.stat().st_size > 65_536:
        raise ValueError("Job JSON must be at most 64 KiB")
    with path.open("r", encoding="utf-8") as handle:
        value = json.load(handle)
    return validate_job(value)


def sample_count(task: dict[str, Any]) -> int:
    return min(int(task["logical_particles"]), int(task["snapshot_limit"]))


def tile_count(task: dict[str, Any]) -> int:
    logical = int(task["logical_particles"])
    tile = int(task["tile_particles"])
    return (logical + tile - 1) // tile


def snapshot_count(task: dict[str, Any]) -> int:
    every = int(task["snapshot_every"])
    steps = int(task["steps"])
    return 2 if every == 0 else 1 + (steps + every - 1) // every


def frame_steps(task: dict[str, Any]) -> list[int]:
    steps = int(task["steps"])
    every = int(task["snapshot_every"])
    if every == 0:
        return [0, steps]
    result = [0]
    step = 0
    while step < steps:
        step += min(every, steps - step)
        result.append(step)
    return result


def sample_ids(task: dict[str, Any]) -> list[int]:
    logical = int(task["logical_particles"])
    count = sample_count(task)
    return [(i * logical) // count for i in range(count)]


def particle_updates(task: dict[str, Any]) -> int:
    multiplier = len(frame_steps(task)) - 1 if task["integrator"] == "circular" else int(task["steps"])
    return int(task["logical_particles"]) * multiplier


def _load_demo() -> list[tuple[float, float, float, float]]:
    rows: list[tuple[float, float, float, float]] = []
    with DEMO_PATH.open(newline="", encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            values = (
                float(row["R_kpc"]),
                float(row["V_gas_kms"]),
                float(row["V_disk_kms"]),
                float(row["V_bul_kms"]),
            )
            if not all(math.isfinite(value) for value in values):
                raise RuntimeError("UFF demo table contains a non-finite CUDA input")
            rows.append(values)
    if not rows:
        raise RuntimeError("UFF demo table must contain at least one row")
    if any(b[0] <= a[0] for a, b in zip(rows, rows[1:])):
        raise RuntimeError("UFF demo radii must be strictly increasing")
    return rows


def _cuda_f32_literal(value: float) -> str:
    text = format(_f32(value), ".9g")
    if "e" not in text.lower() and "." not in text:
        text += ".0"
    return f"{text}f"


def _cuda_demo_constants(rows: list[tuple[float, float, float, float]]) -> str:
    radii = ", ".join(_cuda_f32_literal(row[0]) for row in rows)
    components = ",\n    ".join(
        ", ".join(_cuda_f32_literal(value) for value in row[1:])
        for row in rows
    )
    count = len(rows)
    return (
        f"#define GALAXY_DEMO_N {count}\n"
        f"__device__ __constant__ float DEMO_R[{count}] = {{ {radii} }};\n"
        f"__device__ __constant__ float DEMO_V[{count * 3}] = {{\n    {components}\n}};"
    )


DEMO = _load_demo()
CUDA_SOURCE = CUDA_SOURCE_TEMPLATE.replace(
    "__GALAXY_DEMO_CONSTANTS__", _cuda_demo_constants(DEMO)
)
if "__GALAXY_DEMO_CONSTANTS__" in CUDA_SOURCE:
    raise RuntimeError("CUDA UFF constants were not generated")


def components_at(radius: float) -> tuple[float, float, float]:
    if radius <= DEMO[0][0]:
        return DEMO[0][1:]
    previous = DEMO[0]
    for current in DEMO[1:]:
        if radius <= current[0]:
            t = (radius - previous[0]) / (current[0] - previous[0])
            return tuple(previous[j] + t * (current[j] - previous[j]) for j in range(1, 4))
        previous = current
    return DEMO[-1][1:]


def host_velocity(radius: float, p: dict[str, Any]) -> float:
    gas, disk, bulge = components_at(radius)
    total = max(0.0, gas * abs(gas) + p["disk_ml"] * disk * disk + p["bulge_ml"] * bulge * bulge)
    total += G * p["black_hole_million"] * 1e6 / radius
    model = MODEL_IDS[p["model"]]
    if model == 2:
        mass = 10.0 ** p["halo_log_mass"]
        rho = 3.0 * 0.07**2 / (8.0 * math.pi * G)
        r200 = (3.0 * mass / (4.0 * math.pi * 200.0 * rho)) ** (1.0 / 3.0)

        def nfw(x: float) -> float:
            if x < 1e-4:
                return 0.5*x*x - (2.0/3.0)*x**3 + 0.75*x**4
            return math.log1p(x) - x/(1.0+x)

        c = p["halo_concentration"]
        total += G * mass * nfw(c * radius / r200) / nfw(c) / radius
    elif model == 3:
        x = radius / p["burkert_core"]
        shape = 4.0/3.0*x**3 if x < 1e-3 else math.log((1+x)**2*(1+x*x)) - 2.0*math.atan(x)
        total += G * math.pi * 10.0**p["burkert_log_density"] * p["burkert_core"]**3 * shape / radius
    elif model == 4 and total > 0.0:
        y = total * 1e6 / (radius * KPC_TO_M) / (p["mond_a0"] * 1e-10)
        total /= -math.expm1(-math.sqrt(y))
    elif model == 5:
        x = radius / p["uff_core"]
        if x < 1e-3:
            base = x*x*(1.0/3.0 - x*x/5.0 + x**4/7.0)
        else:
            base = max(0.0, 1.0 - math.atan(x)/x)
        total += p["uff_v_inf"]**2 * base * math.exp(2.0*p["uff_beta"]*x/(1.0+x))
    return math.sqrt(total)


def initialize_particle(task: dict[str, Any], index: int) -> list[float]:
    p = task["physics"]
    u = random_global(index, int(task["seed"]), 0)
    v = random_global(index, int(task["seed"]), 1)
    w = random_global(index, int(task["seed"]), 2)
    kind = random_global(index, int(task["seed"]), 4)
    bulge = kind < _f32(task["bulge_fraction"])
    halo = kind >= 0.97
    if bulge:
        r = 12.0 * (0.015 + 0.27 * u**1.8)
    elif halo:
        r = 12.0 * (0.25 + 0.85 * u)
    else:
        r = 12.0 * (0.06 + 0.92 * u**1.4)
    if bulge or halo:
        theta = TAU * v
    else:
        theta = math.floor(v * task["arms"]) * TAU / task["arms"]
        theta += math.log(r / 1.2) / math.tan(math.radians(task["pitch_deg"]))
        theta += (w - 0.5) * task["scatter"]
    if bulge:
        height = 2.28 * (1.0 - r/3.6)
    elif halo:
        height = 4.8
    else:
        height = task["thickness_kpc"] * (0.4 + r/12.0)
    z = (random_global(index, int(task["seed"]), 3) - 0.5) * 2.0 * height
    if task["integrator"] == "leapfrog":
        soft_r = math.sqrt(r*r + task["softening_kpc"]**2)
        speed = host_velocity(soft_r, p) * r/soft_r
    else:
        speed = host_velocity(r, p)
    speed *= KMS_TO_KPC_MYR
    omega = task["direction"] * speed / r
    kick = task["radial_kick_kms"] * KMS_TO_KPC_MYR
    return [_f32(x) for x in (
        r, theta, z, omega,
        r*math.cos(theta), r*math.sin(theta),
        -omega*r*math.sin(theta)+kick*math.cos(theta),
        omega*r*math.cos(theta)+kick*math.sin(theta),
    )]


def _load_cupy():
    try:
        import cupy as cp
        import numpy as np
    except Exception as exc:
        raise RuntimeError(
            "CUDA backend requires CuPy. Run `bash scripts/bootstrap-cuda.sh` once; "
            "it installs the pinned CUDA 12/13 wheel into .galaxy-cuda-python/."
        ) from exc
    try:
        count = cp.cuda.runtime.getDeviceCount()
    except Exception as exc:
        raise RuntimeError(
            "CuPy is installed but CUDA initialization failed. Confirm `nvidia-smi` works "
            "and that the NVIDIA device is visible inside this session/container."
        ) from exc
    if count <= 0:
        raise RuntimeError("No CUDA devices are visible")
    return cp, np


def _decode_name(value: Any) -> str:
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace").rstrip("\x00")
    return str(value)


def devices() -> list[dict[str, Any]]:
    cp, _ = _load_cupy()
    result = []
    driver = int(cp.cuda.runtime.driverGetVersion())
    runtime = int(cp.cuda.runtime.runtimeGetVersion())
    cupy_version = str(cp.__version__)
    for index in range(cp.cuda.runtime.getDeviceCount()):
        props = cp.cuda.runtime.getDeviceProperties(index)
        name = _decode_name(props.get("name", f"CUDA device {index}"))
        with cp.cuda.Device(index):
            free, total_mem = cp.cuda.runtime.memGetInfo()
        result.append({
            "index": index,
            "name": name,
            "backend": "CUDA",
            "device_type": "DiscreteGpu",
            "driver": "NVIDIA CUDA Driver",
            "driver_version": driver,
            "runtime_version": runtime,
            "cupy_version": cupy_version,
            "software": False,
            "compute_capability": f"{int(props.get('major', 0))}.{int(props.get('minor', 0))}",
            "total_memory_bytes": int(total_mem),
            "free_memory_bytes": int(free),
        })
    return result


def select_device(query: str | None) -> dict[str, Any]:
    found = devices()
    if not found:
        raise RuntimeError("No CUDA devices are visible")
    if query is not None:
        try:
            index = int(query)
        except ValueError:
            needle = query.lower()
            matches = [item for item in found if needle in item["name"].lower()]
            if not matches:
                raise RuntimeError(f"No CUDA device name contains {query!r}")
            return matches[0]
        if index < 0 or index >= len(found):
            raise RuntimeError(f"CUDA adapter index {index} is out of range")
        return found[index]
    return found[0]


def leapfrog_steps_per_launch(resident_particles: int) -> int:
    if resident_particles <= 0:
        raise ValueError("resident_particles must be positive")
    by_particle_updates = max(
        1,
        MAX_LEAPFROG_PARTICLE_UPDATES_PER_LAUNCH // resident_particles,
    )
    return min(MAX_LEAPFROG_STEPS_PER_LAUNCH, by_particle_updates)


class CudaGpu:
    def __init__(self, adapter: str | None):
        self.cp, self.np = _load_cupy()
        self.info = select_device(adapter)
        self.device = self.cp.cuda.Device(int(self.info["index"]))
        self.device.use()
        self.module = self.cp.RawModule(
            code=CUDA_SOURCE,
            options=("--std=c++11",),
            name_expressions=("initialize", "circular", "leapfrog", "gather"),
        )
        self.k_initialize = self.module.get_function("initialize")
        self.k_circular = self.module.get_function("circular")
        self.k_leapfrog = self.module.get_function("leapfrog")
        self.k_gather = self.module.get_function("gather")

    def _physics_args(self, task: dict[str, Any]) -> tuple[Any, ...]:
        np = self.np
        p = task["physics"]
        return (
            np.uint32(MODEL_IDS[p["model"]]),
            np.float32(p["disk_ml"]), np.float32(p["bulge_ml"]), np.float32(p["black_hole_million"]),
            np.float32(p["uff_v_inf"]), np.float32(p["uff_core"]), np.float32(p["uff_beta"]),
            np.float32(p["halo_log_mass"]), np.float32(p["halo_concentration"]),
            np.float32(p["burkert_log_density"]), np.float32(p["burkert_core"]), np.float32(p["mond_a0"]),
        )

    def _launch(self, kernel, count: int, args: tuple[Any, ...]) -> None:
        if count <= 0:
            return
        kernel(((count + 255)//256,), (256,), args)

    def _timed(self, function) -> float:
        start = self.cp.cuda.Event()
        end = self.cp.cuda.Event()
        start.record()
        function()
        end.record()
        end.synchronize()
        return float(self.cp.cuda.get_elapsed_time(start, end)) / 1000.0

    def initialize_tile(self, task: dict[str, Any], base: int, count: int, local_indices: list[int]):
        cp, np = self.cp, self.np
        try:
            particles = cp.empty((count, 8), dtype=cp.float32)
        except Exception as exc:
            raise RuntimeError(
                f"Unable to allocate {count * 32} resident particle bytes; reduce tile_particles"
            ) from exc
        base_lo = base & 0xFFFFFFFF
        base_hi = (base >> 32) & 0xFFFFFFFF
        physics = self._physics_args(task)
        args = (
            particles,
            np.uint32(count), np.uint32(task["seed"]), np.uint32(base_lo), np.uint32(base_hi),
            *physics,
            np.uint32(1 if task["integrator"] == "leapfrog" else 0),
            np.float32(task["softening_kpc"]),
            np.float32(task["arms"]), np.float32(math.radians(task["pitch_deg"])),
            np.float32(task["scatter"]), np.float32(task["bulge_fraction"]),
            np.float32(task["thickness_kpc"]), np.float32(task["direction"]),
            np.float32(task["radial_kick_kms"]),
        )
        elapsed = self._timed(lambda: self._launch(self.k_initialize, count, args))
        indices = cp.asarray(local_indices or [0], dtype=cp.uint32)
        return {
            "particles": particles,
            "indices": indices,
            "sample_count": len(local_indices),
            "count": count,
            "base": base,
        }, elapsed

    def advance(self, field: dict[str, Any], task: dict[str, Any], repeats: int, total: int) -> float:
        np = self.np
        count = int(field["count"])
        if task["integrator"] == "circular":
            phases = phase_time_parts(total * float(task["dt_myr"]))
            args = (
                field["particles"], np.uint32(count),
                np.float32(phases[0]), np.float32(phases[1]), np.float32(phases[2]), np.float32(phases[3]),
            )
            return self._timed(lambda: self._launch(self.k_circular, count, args))

        physics = self._physics_args(task)
        chunk_steps = leapfrog_steps_per_launch(count)

        def launch_chunks() -> None:
            remaining = repeats
            while remaining > 0:
                chunk = min(remaining, chunk_steps)
                args = (
                    field["particles"], np.uint32(count), np.uint32(chunk),
                    np.float32(task["dt_myr"]), np.float32(task["softening_kpc"]),
                    *physics,
                )
                self._launch(self.k_leapfrog, count, args)
                remaining -= chunk

        return self._timed(launch_chunks)

    def sample(self, field: dict[str, Any]) -> Any:
        sample_n = int(field["sample_count"])
        if sample_n == 0:
            return self.np.empty((0, 8), dtype=self.np.float32)
        cp, np = self.cp, self.np
        output = cp.empty((sample_n, 8), dtype=cp.float32)
        args = (field["particles"], field["indices"], output, np.uint32(sample_n))
        self._launch(self.k_gather, sample_n, args)
        self.cp.cuda.runtime.deviceSynchronize()
        host = cp.asnumpy(output)
        if host.dtype != np.float32 or host.shape != (sample_n, 8):
            raise RuntimeError("CUDA gather returned an unexpected packed sample layout")
        if not np.isfinite(host).all():
            raise RuntimeError("CUDA produced a non-finite tiled result; reduce dt or check parameters")
        return host


def phase_time_parts(time_myr: float) -> list[float]:
    remaining = time_myr / TAU
    parts: list[float] = []
    for _ in range(4):
        bits = struct.unpack("<Q", struct.pack("<d", remaining))[0]
        high_bits = bits & ~((1 << 41) - 1)
        high = struct.unpack("<d", struct.pack("<Q", high_bits))[0]
        parts.append(_f32(high))
        remaining -= high
    return parts


def _write_json(path: Path, value: Any) -> None:
    with path.open("w", encoding="utf-8") as handle:
        json.dump(value, handle, indent=2, sort_keys=False)
        handle.write("\n")


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(64 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def runtime_source_sha256() -> str:
    digest = hashlib.sha256()
    for path in RUNTIME_SOURCE_PATHS:
        digest.update(path.relative_to(ROOT).as_posix().encode())
        digest.update(b"\0")
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(64 * 1024), b""):
                digest.update(chunk)
        digest.update(b"\0")
    return digest.hexdigest()


def artifacts(directory: Path) -> list[dict[str, Any]]:
    result = []
    for path in sorted(directory.iterdir()):
        if not path.is_file() or path.name == "receipt.json":
            continue
        result.append({"file": path.name, "bytes": path.stat().st_size, "sha256": _sha256_file(path)})
    return result


def _png_chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


def write_rgb_png(path: Path, width: int, height: int, pixels: bytes) -> None:
    if len(pixels) != width * height * 3:
        raise ValueError("RGB pixel payload length mismatch")
    raw = b"".join(b"\x00" + pixels[y*width*3:(y+1)*width*3] for y in range(height))
    payload = b"\x89PNG\r\n\x1a\n"
    payload += _png_chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    payload += _png_chunk(b"IDAT", zlib.compress(raw, level=6))
    payload += _png_chunk(b"IEND", b"")
    path.write_bytes(payload)


def write_snapshot(directory: Path, task: dict[str, Any], step: int, ids: list[int], points: Any) -> dict[str, Any]:
    if len(ids) != len(points) or len(points) != sample_count(task):
        raise RuntimeError("Invalid CUDA u64 snapshot values, ids, or sample length")
    if any(not math.isfinite(float(v)) for point in points for v in point):
        raise RuntimeError("Invalid CUDA u64 snapshot values, ids, or sample length")

    stem = f"frame-{step:06d}"
    max_radius = 0.0
    max_lz_drift = 0.0
    with (directory / f"{stem}.csv").open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle, lineterminator="\n")
        writer.writerow(["particle_id","x_kpc","y_kpc","z_kpc","vx_kms","vy_kms","initial_radius_kpc"])
        for particle_id, p in zip(ids, points):
            r0, _, z, omega, x, y, vx, vy = map(float, p)
            initial_lz = r0*r0*omega
            max_radius = max(max_radius, math.hypot(x, y))
            if initial_lz != 0.0:
                max_lz_drift = max(max_lz_drift, abs((x*vy-y*vx-initial_lz)/initial_lz))
            writer.writerow([
                particle_id,
                f"{x:.9f}", f"{y:.9f}", f"{z:.9f}",
                f"{vx/KMS_TO_KPC_MYR:.9f}", f"{vy/KMS_TO_KPC_MYR:.9f}",
                f"{r0:.9f}",
            ])

    size = int(task["image_size"])
    light = [0.0] * (size * size)
    tilt = math.radians(float(task["inclination_deg"]))
    cos_t, sin_t = math.cos(tilt), math.sin(tilt)
    extent = float(task["extent_kpc"])
    for p in points:
        x = float(p[4])
        y = float(p[5]) * cos_t + float(p[2]) * sin_t
        px = int((x/extent + 1.0)*0.5*size)
        py = int((y/extent + 1.0)*0.5*size)
        for dy in (-1,0,1):
            for dx in (-1,0,1):
                xx, yy = px+dx, py+dy
                if 0 <= xx < size and 0 <= yy < size:
                    light[yy*size+xx] += 0.7 if dx == 0 and dy == 0 else 0.12
    pixels = bytearray()
    for value in light:
        value = 1.0 - math.exp(-value)
        pixels.extend((
            int(3.0 + 237.0*value),
            int(5.0 + 211.0*value),
            int(9.0 + 166.0*value),
        ))
    write_rgb_png(directory / f"{stem}.png", size, size, bytes(pixels))
    return {
        "step": step,
        "time_myr": step * float(task["dt_myr"]),
        "image": f"{stem}.png",
        "csv": f"{stem}.csv",
        "sampled_particles": len(points),
        "max_sampled_radius_kpc": max_radius,
        "max_sampled_relative_lz_drift": max_lz_drift,
    }


def write_viewer(directory: Path, frames: list[dict[str, Any]], particles: int) -> None:
    data = json.dumps(frames, separators=(",", ":"))
    html = f"""<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>GALAXY u64 CUDA run</title>
<style>body{{margin:0;background:#080a0d;color:#e6e4df;font:15px system-ui;max-width:1100px;padding:24px;margin:auto}}h1{{font-weight:500;letter-spacing:.12em}}img{{display:block;width:min(100%,850px);margin:20px auto}}input{{width:70%;accent-color:#d7b47c}}button{{padding:8px 16px;background:#292219;color:#efcca0;border:1px solid #745b35;cursor:pointer}}p{{color:#a2a6ae}}a{{color:#d7b47c}}</style>
<h1>GALAXY / U64 CUDA RUN</h1><p>{particles} logical particles · bounded resident CUDA tiles · preview shows the globally sampled export</p><img id="frame" alt="Galaxy particle positions"><button id="play">Play</button> <input id="step" aria-label="Snapshot" type="range" min="0" max="{max(0,len(frames)-1)}" value="0"><p id="caption"></p><a href="receipt.json">Run report</a> · <a href="job.json">Resolved job</a>
<script>const frames={data};const slider=document.getElementById('step');let timer;function show(){{const f=frames[Number(slider.value)];document.getElementById('frame').src=f.image;document.getElementById('caption').textContent='Step '+f.step+' · '+f.time_myr.toFixed(2)+' Myr · '+f.sampled_particles+' globally sampled particles';}}slider.oninput=show;document.getElementById('play').onclick=function(){{if(timer){{clearInterval(timer);timer=null;this.textContent='Play';}}else{{this.textContent='Pause';timer=setInterval(()=>{{slider.value=(Number(slider.value)+1)%frames.length;show();}},250);}}}};show();</script></html>"""
    (directory / "viewer.html").write_text(html, encoding="utf-8")


def cuda_kernel_schedule(task: dict[str, Any]) -> str:
    if task["integrator"] == "circular":
        return "analytic circular phase kernel launched once per requested frame interval"
    return (
        "independent per-particle leapfrog steps split across bounded CUDA launches; "
        f"at most {MAX_LEAPFROG_STEPS_PER_LAUNCH} steps and "
        f"{MAX_LEAPFROG_PARTICLE_UPDATES_PER_LAUNCH} resident particle-updates per launch"
    )


def force_model_description(task: dict[str, Any]) -> str:
    model = task["physics"]["model"]
    if task["integrator"] == "circular":
        return f"prescribed circular speed from {model} velocity law"
    return (
        f"planar fixed radial acceleration derived from {model} circular-speed law; "
        "softened radius; static authored height"
    )


def execute(job: dict[str, Any], directory: Path, gpu: CudaGpu) -> dict[str, Any]:
    task = job["task"]
    started = time.perf_counter()
    steps = frame_steps(task)
    ids = sample_ids(task)
    samples = len(ids)
    np = gpu.np
    try:
        cache = np.empty((len(steps), samples, 8), dtype=np.float32)
        filled = np.zeros((len(steps), samples), dtype=np.bool_)
    except (MemoryError, ValueError) as exc:
        payload = len(steps) * samples * 32
        raise RuntimeError(
            f"Unable to allocate the packed {payload}-byte CUDA sample cache; "
            "reduce snapshot_limit or snapshot count"
        ) from exc
    tiles = tile_count(task)
    sample_cursor = 0
    initialization_seconds = 0.0
    integration_seconds = 0.0
    progress_every = max(tiles // 20, 1)

    for tile in range(tiles):
        base = tile * int(task["tile_particles"])
        count = min(int(task["logical_particles"]) - base, int(task["tile_particles"]))
        end = base + count
        local_indices: list[int] = []
        slots: list[int] = []
        while sample_cursor < len(ids) and ids[sample_cursor] < end:
            particle_id = ids[sample_cursor]
            if particle_id < base:
                raise RuntimeError("global sample plan moved backwards across a tile")
            local_indices.append(particle_id - base)
            slots.append(sample_cursor)
            sample_cursor += 1

        field, init_seconds = gpu.initialize_tile(task, base, count, local_indices)
        initialization_seconds += init_seconds
        previous = 0
        for frame_index, step in enumerate(steps):
            if step > previous:
                integration_seconds += gpu.advance(field, task, step - previous, step)
                previous = step
            if local_indices:
                points = gpu.sample(field)
                if points.shape != (len(slots), 8):
                    raise RuntimeError("CUDA tile gather returned the wrong number of global samples")
                cache[frame_index, slots, :] = points
                filled[frame_index, slots] = True

        del field
        gpu.cp.get_default_memory_pool().free_all_blocks()
        if tile < 2 or tile + 1 == tiles or (tile + 1) % progress_every == 0:
            print(
                f"tile {tile+1}/{tiles} · global ids {base}..{end-1} · "
                f"{count} resident · {len(local_indices)} global samples",
                file=sys.stderr,
            )

    if sample_cursor != len(ids):
        raise RuntimeError("not every planned global sample was assigned to a resident tile")

    frames = []
    for frame_index, step in enumerate(steps):
        if not bool(filled[frame_index].all()):
            raise RuntimeError("missing global sample after tiled CUDA execution")
        points = cache[frame_index]
        frames.append(write_snapshot(directory, task, step, ids, points))
        print(
            f"snapshot {frame_index+1}/{len(steps)} · step {step}/{task['steps']} · "
            f"{step*task['dt_myr']:.2f} Myr · {len(points)} globally sampled",
            file=sys.stderr,
        )

    write_viewer(directory, frames, int(task["logical_particles"]))
    updates = particle_updates(task)
    throughput = updates / integration_seconds if integration_seconds > 0.0 else 0.0
    sample_cache_entries = len(steps) * samples
    return {
        "adapter": gpu.info,
        "arithmetic": "float32 CUDA kernels; split-u64-compatible global addressing; float64 host diagnostics",
        "addressing": "split-u64-hash32-avalanche-v1",
        "cuda_kernel_schedule": cuda_kernel_schedule(task),
        "cuda_leapfrog_max_steps_per_launch": MAX_LEAPFROG_STEPS_PER_LAUNCH,
        "cuda_leapfrog_max_particle_updates_per_launch": MAX_LEAPFROG_PARTICLE_UPDATES_PER_LAUNCH,
        "logical_particles": int(task["logical_particles"]),
        "resident_tile_particles": min(int(task["logical_particles"]), int(task["tile_particles"])),
        "resident_particle_bytes": min(int(task["logical_particles"]), int(task["tile_particles"])) * 32,
        "tiles": tiles,
        "snapshot_limit": sample_count(task),
        "sample_index_math": "exact Python integer mapping into the u64 logical index space",
        "sample_cache_entries": sample_cache_entries,
        "sample_cache_particle_payload_bytes": int(cache.nbytes),
        "sample_cache_validity_bytes": int(filled.nbytes),
        "sample_cache_host_representation": "packed NumPy float32 array plus boolean validity bitmap",
        "simulated_time_myr": int(task["steps"]) * float(task["dt_myr"]),
        "particle_updates": updates,
        "initialization_compute_wall_seconds": initialization_seconds,
        "integration_compute_wall_seconds": integration_seconds,
        "integration_particle_updates_per_second": throughput,
        "execution_wall_seconds": time.perf_counter() - started,
        "frames": frames,
        "force_model": force_model_description(task),
        "partition_semantics": (
            "independent test particles in one fixed potential; tiles are an execution partition, "
            "not a physics approximation"
        ),
    }


def verify_u64(gpu: CudaGpu | None) -> dict[str, Any]:
    ids = [(1 << 32) - 1, 1 << 32, (1 << 32) + 1]
    fingerprints = [address_fingerprint(i, 303) for i in ids]
    if len(set(fingerprints)) != 3:
        raise RuntimeError("64-bit addressing aliases at the 2^32 boundary")

    gpu_checked = False
    if gpu is not None:
        job = validate_job({
            "schema_version": 1,
            "task": {
                "logical_particles": (1 << 32) + 2,
                "tile_particles": 4,
                "snapshot_limit": 3,
                "steps": 1,
            },
        })
        task = job["task"]
        base = (1 << 32) - 1
        field, _ = gpu.initialize_tile(task, base, 3, [0, 1, 2])
        actual = gpu.sample(field)
        for result, particle_id in zip(actual, ids):
            expected = initialize_particle(task, particle_id)
            for a, b in zip(result, expected):
                if not math.isfinite(float(a)) or abs(float(a)-b) > 4e-4*(1.0+abs(b)):
                    raise RuntimeError(
                        f"CUDA u64 boundary initialization mismatch at id {particle_id}: {a} vs {b}"
                    )
        gpu_checked = True
    return {
        "status": "passed",
        "backend": "CUDA" if gpu is not None else "host",
        "addressing": "split-u64-hash32-avalanche-v1",
        "boundary": "2^32",
        "boundary_ids": ids,
        "address_fingerprints": fingerprints,
        "distinct_boundary_addresses": True,
        "gpu_boundary_checked": gpu_checked,
        "adapter": gpu.info if gpu is not None else None,
    }


def requested_backend() -> str:
    requested = os.environ.get("GALAXY_BACKEND_REQUESTED", "cuda").lower()
    if requested not in {"auto", "cuda"}:
        raise RuntimeError(
            f"CUDA runtime received invalid requested backend provenance {requested!r}; expected auto or cuda"
        )
    return requested


def run_job(job_path: Path, directory: Path, adapter: str | None) -> None:
    job = load_job(job_path)
    directory.parent.mkdir(parents=True, exist_ok=True)
    directory.mkdir()
    _write_json(directory / "job.json", job)
    provenance = json.loads(PROVENANCE_PATH.read_text(encoding="utf-8"))
    receipt: dict[str, Any] = {
        "schema_version": 1,
        "runtime": "galaxy-u64-cuda",
        "runtime_package_version": "0.3.0-cuda1",
        "status": "running",
        "runtime_source_sha256": runtime_source_sha256(),
        "job_sha256": _sha256_file(directory / "job.json"),
        "uff_source": provenance,
        "backend_requested": requested_backend(),
        "backend_selected": "cuda",
        "allow_software": False,
    }
    _write_json(directory / "receipt.json", receipt)
    try:
        gpu = CudaGpu(adapter)
        print(f"Engine: {gpu.info['name']} (CUDA)", file=sys.stderr)
        receipt["cuda_dependency"] = {"cupy_version": gpu.info["cupy_version"]}
        receipt["results"] = execute(job, directory, gpu)
        receipt["artifacts"] = artifacts(directory)
        receipt["status"] = "complete"
        _write_json(directory / "receipt.json", receipt)
    except Exception as exc:
        receipt["status"] = "failed"
        receipt["error"] = str(exc)
        _write_json(directory / "receipt.json", receipt)
        raise
    print(f"Completed: {directory}")


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="GALAXY memory-bounded u64 CUDA backend")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("devices", help="List CUDA compute devices")

    validate = sub.add_parser("validate", help="Validate and print a resolved u64 tiled job")
    validate.add_argument("--job", type=Path, required=True)

    verify = sub.add_parser("verify", help="Prove distinct addressing across 2^32, optionally on CUDA")
    verify.add_argument("--cpu", action="store_true", help="Run only the host-side boundary discriminator")
    verify.add_argument("--adapter", help="CUDA device index or case-insensitive name substring")

    run = sub.add_parser("run", help="Execute one logical population through bounded CUDA tiles")
    run.add_argument("--job", type=Path, required=True)
    run.add_argument("--output", type=Path, required=True)
    run.add_argument("--adapter", help="CUDA device index or case-insensitive name substring")
    return parser


def main() -> int:
    try:
        args = _parser().parse_args()
        if args.command == "devices":
            print(json.dumps(devices(), indent=2))
        elif args.command == "validate":
            print(json.dumps(load_job(args.job), indent=2))
        elif args.command == "verify":
            gpu = None if args.cpu else CudaGpu(args.adapter)
            print(json.dumps(verify_u64(gpu), indent=2))
        elif args.command == "run":
            run_job(args.job, args.output, args.adapter)
        else:
            raise AssertionError(args.command)
        return 0
    except Exception as exc:
        print(f"GALAXY-U64-CUDA: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
