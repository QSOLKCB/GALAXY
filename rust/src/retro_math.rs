// SPDX-License-Identifier: Apache-2.0
//! Deterministic reference math inspired by Elite's compact galaxy seed
//! recurrence and Doom's fixed-point/BAM arithmetic. This module is a
//! validation oracle; it does not replace GALAXY's physical dynamics.

pub const ELITE_GALAXY1: [u16; 3] = [0x5a4a, 0x0248, 0xb753];
pub const BAM_QUARTER: u32 = 0x4000_0000;
pub const BAM_HALF: u32 = 0x8000_0000;

const CORDIC_K_Q30: i64 = 652_032_874;
const CORDIC_ATAN_BAM: [i64; 24] = [
    536_870_912, 316_933_406, 167_458_907, 85_004_756, 42_667_331, 21_354_465,
    10_679_838, 5_340_245, 2_670_163, 1_335_087, 667_544, 333_772,
    166_886, 83_443, 41_722, 20_861, 10_430, 5_215, 2_608, 1_304,
    652, 326, 163, 81,
];

type Matrix = [[u32; 3]; 3];

pub fn elite_twist(seed: [u16; 3]) -> [u16; 3] {
    [seed[1], seed[2], seed[0].wrapping_add(seed[1]).wrapping_add(seed[2])]
}

fn matrix_mul(a: Matrix, b: Matrix) -> Matrix {
    let mut out = [[0_u32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let mut sum = 0_u64;
            for k in 0..3 {
                sum += a[i][k] as u64 * b[k][j] as u64;
            }
            out[i][j] = (sum & 0xffff) as u32;
        }
    }
    out
}

fn matrix_pow(mut steps: u64) -> Matrix {
    let mut result = [[1, 0, 0], [0, 1, 0], [0, 0, 1]];
    let mut base = [[0, 1, 0], [0, 0, 1], [1, 1, 1]];
    while steps != 0 {
        if steps & 1 != 0 {
            result = matrix_mul(result, base);
        }
        base = matrix_mul(base, base);
        steps >>= 1;
    }
    result
}

pub fn elite_jump(seed: [u16; 3], steps: u64) -> [u16; 3] {
    let matrix = matrix_pow(steps);
    let mut out = [0_u16; 3];
    for i in 0..3 {
        let sum = (0..3).map(|j| matrix[i][j] as u64 * seed[j] as u64).sum::<u64>();
        out[i] = (sum & 0xffff) as u16;
    }
    out
}

fn rol32(value: u32, shift: u32) -> u32 {
    value.rotate_left(shift)
}

/// Elite's three-word recurrence supplies the low-32 procedural grammar.
/// High sector bits are deliberately mixed separately by `sector_salt` so the
/// reference layer does not inherit the recurrence's 2^32 repetition.
pub fn sector_seed(global_seed: u32, sector: u64) -> [u16; 3] {
    let base = [
        ELITE_GALAXY1[0] ^ global_seed as u16,
        ELITE_GALAXY1[1].wrapping_add((global_seed >> 16) as u16),
        ELITE_GALAXY1[2] ^ rol32(global_seed, 7) as u16,
    ];
    elite_jump(base, sector as u32 as u64)
}

/// A u32 salt suitable for XORing into GALAXY's existing particle mixer.
/// The Elite-like state is not used as a particle identity and cannot collapse
/// the full-u64 domain: both halves of the sector index feed the avalanche hash.
pub fn sector_salt(global_seed: u32, sector: u64) -> u32 {
    let state = sector_seed(global_seed, sector);
    let folded = (((state[0] as u32) << 16) | state[1] as u32) ^ rol32(state[2] as u32, 11);
    let lo = sector as u32;
    let hi = (sector >> 32) as u32;
    crate::hash32(folded ^ crate::hash32(lo ^ 0x9e37_79b9) ^ crate::hash32(hi ^ 0x85eb_ca6b))
}

pub fn bam_add(angle: u32, delta: u32) -> u32 {
    angle.wrapping_add(delta)
}

pub fn bam_sub(angle: u32, delta: u32) -> u32 {
    angle.wrapping_sub(delta)
}

pub fn bam_advance(angle: u32, delta: u32, ticks: u64) -> u32 {
    angle.wrapping_add(delta.wrapping_mul(ticks as u32))
}

pub fn bam_retreat(angle: u32, delta: u32, ticks: u64) -> u32 {
    angle.wrapping_sub(delta.wrapping_mul(ticks as u32))
}

/// Return cosine/sine in signed Q2.30 using only integer arithmetic after the
/// input BAM32 angle has been supplied. The constants are committed and shared
/// conceptually with the JavaScript implementation, making this a deterministic
/// cross-language reference rather than a fast production trig path.
pub fn sin_cos_q30(angle: u32) -> (i64, i64) {
    let mut z = if angle < BAM_HALF {
        angle as i64
    } else {
        angle as i64 - (1_i64 << 32)
    };
    let mut x = CORDIC_K_Q30;
    let mut y = 0_i64;
    if z > BAM_QUARTER as i64 {
        z -= BAM_HALF as i64;
        x = -x;
    } else if z < -(BAM_QUARTER as i64) {
        z += BAM_HALF as i64;
        x = -x;
    }
    for (i, atan) in CORDIC_ATAN_BAM.iter().enumerate() {
        let old_x = x;
        if z >= 0 {
            x -= y >> i;
            y += old_x >> i;
            z -= *atan;
        } else {
            x += y >> i;
            y -= old_x >> i;
            z += *atan;
        }
    }
    (x, y)
}

/// Project a signed Q16.16 radius with the integer CORDIC reference.
pub fn project_q16(radius_q16: i32, angle: u32) -> (i64, i64) {
    let (cos, sin) = sin_cos_q30(angle);
    let r = radius_q16 as i64;
    ((r * cos) >> 30, (r * sin) >> 30)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jump_matches_repeated_twists() {
        let mut state = ELITE_GALAXY1;
        for steps in 0..1024_u64 {
            assert_eq!(elite_jump(ELITE_GALAXY1, steps), state);
            state = elite_twist(state);
        }
    }

    #[test]
    fn high_sector_bits_do_not_alias_in_salt() {
        assert_eq!(sector_seed(303, 0), sector_seed(303, 1_u64 << 32));
        assert_ne!(sector_salt(303, 0), sector_salt(303, 1_u64 << 32));
    }

    #[test]
    fn bam_round_trip_is_exact() {
        let start = 0x1234_5678;
        let delta = 0x00ab_cdef;
        let ticks = 1_000_003;
        assert_eq!(bam_retreat(bam_advance(start, delta, ticks), delta, ticks), start);
    }

    #[test]
    fn cardinal_cordic_vectors_are_stable() {
        assert_eq!(sin_cos_q30(0), (1_073_741_826, -76));
        assert_eq!(sin_cos_q30(BAM_QUARTER), (-76, 1_073_741_826));
        assert_eq!(sin_cos_q30(BAM_HALF), (-1_073_741_830, 79));
    }
}
