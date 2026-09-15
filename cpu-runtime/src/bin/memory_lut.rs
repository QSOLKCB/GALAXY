// SPDX-License-Identifier: Apache-2.0
use galaxy_retro_math::sin_cos_q30;
use std::{collections::BTreeSet, mem::size_of};

pub const LUT_BITS: u32 = 14;
pub const LUT_SIZE: usize = 1 << LUT_BITS;
const LUT_SHIFT: u32 = 32 - LUT_BITS;
const LUT_FRAC_MASK: u32 = (1_u32 << LUT_SHIFT) - 1;
const LUT_QUARTER: usize = LUT_SIZE / 4;
const LUT_MASK: usize = LUT_SIZE - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LutMode {
    Full,
    CosineCorrected,
    QuarterCorrected,
}

impl LutMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "full" => Ok(Self::Full),
            "cosine" | "cosine-corrected" => Ok(Self::CosineCorrected),
            "quarter" | "quarter-corrected" => Ok(Self::QuarterCorrected),
            _ => Err("--lut must be full, cosine, or quarter".into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Full => "full-sine-cosine-16k",
            Self::CosineCorrected => "cosine-plus-exact-sine-correction-v1",
            Self::QuarterCorrected => "quarter-cosine-plus-exact-correction-palette-v1",
        }
    }
}

#[derive(Debug)]
enum LutStorage {
    Full {
        cos: Vec<i32>,
        sin: Vec<i32>,
    },
    CosineCorrected {
        cos: Vec<i32>,
        sin_correction: Vec<i16>,
    },
    QuarterCorrected {
        quarter_cos: Vec<i32>,
        correction_palette: Vec<i16>,
        cos_code: Vec<u8>,
        sin_code: Vec<u8>,
    },
}

#[derive(Debug)]
pub struct Lut {
    mode: LutMode,
    storage: LutStorage,
}

fn canonical_sample(index: usize) -> (i32, i32) {
    let angle = (index as u32) << LUT_SHIFT;
    let (c, s) = sin_cos_q30(angle);
    (c as i32, s as i32)
}

fn checked_i16(value: i64, label: &str) -> Result<i16, String> {
    i16::try_from(value).map_err(|_| format!("{label} correction {value} does not fit i16"))
}

fn quarter_cos_approx(quarter_cos: &[i32], index: usize) -> i64 {
    let index = index & LUT_MASK;
    let quadrant = index / LUT_QUARTER;
    let offset = index % LUT_QUARTER;
    match quadrant {
        0 => quarter_cos[offset] as i64,
        1 => -(quarter_cos[LUT_QUARTER - offset] as i64),
        2 => -(quarter_cos[offset] as i64),
        3 => quarter_cos[LUT_QUARTER - offset] as i64,
        _ => unreachable!(),
    }
}

impl Lut {
    pub fn build(mode: LutMode) -> Result<Self, String> {
        let storage = match mode {
            LutMode::Full => {
                let mut cos = Vec::with_capacity(LUT_SIZE);
                let mut sin = Vec::with_capacity(LUT_SIZE);
                for index in 0..LUT_SIZE {
                    let (c, s) = canonical_sample(index);
                    cos.push(c);
                    sin.push(s);
                }
                LutStorage::Full { cos, sin }
            }
            LutMode::CosineCorrected => {
                let mut cos = Vec::with_capacity(LUT_SIZE);
                for index in 0..LUT_SIZE {
                    cos.push(canonical_sample(index).0);
                }
                let mut sin_correction = Vec::with_capacity(LUT_SIZE);
                for index in 0..LUT_SIZE {
                    let canonical_sin = canonical_sample(index).1 as i64;
                    let shifted_cos = cos[(index + LUT_SIZE - LUT_QUARTER) & LUT_MASK] as i64;
                    sin_correction.push(checked_i16(
                        canonical_sin - shifted_cos,
                        "cosine-only sine",
                    )?);
                }
                LutStorage::CosineCorrected {
                    cos,
                    sin_correction,
                }
            }
            LutMode::QuarterCorrected => {
                let mut quarter_cos = Vec::with_capacity(LUT_QUARTER + 1);
                for index in 0..=LUT_QUARTER {
                    quarter_cos.push(canonical_sample(index).0);
                }

                let mut unique = BTreeSet::<i16>::new();
                for index in 0..LUT_SIZE {
                    let (canonical_cos, canonical_sin) = canonical_sample(index);
                    let approx_cos = quarter_cos_approx(&quarter_cos, index);
                    let approx_sin = quarter_cos_approx(
                        &quarter_cos,
                        (index + LUT_SIZE - LUT_QUARTER) & LUT_MASK,
                    );
                    unique.insert(checked_i16(
                        canonical_cos as i64 - approx_cos,
                        "quarter cosine",
                    )?);
                    unique.insert(checked_i16(
                        canonical_sin as i64 - approx_sin,
                        "quarter sine",
                    )?);
                }
                let correction_palette: Vec<i16> = unique.into_iter().collect();
                if correction_palette.len() > 256 {
                    return Err(format!(
                        "quarter correction palette requires {} entries",
                        correction_palette.len()
                    ));
                }

                let mut cos_code = Vec::with_capacity(LUT_SIZE);
                let mut sin_code = Vec::with_capacity(LUT_SIZE);
                for index in 0..LUT_SIZE {
                    let (canonical_cos, canonical_sin) = canonical_sample(index);
                    let approx_cos = quarter_cos_approx(&quarter_cos, index);
                    let approx_sin = quarter_cos_approx(
                        &quarter_cos,
                        (index + LUT_SIZE - LUT_QUARTER) & LUT_MASK,
                    );
                    let dc = checked_i16(canonical_cos as i64 - approx_cos, "quarter cosine")?;
                    let ds = checked_i16(canonical_sin as i64 - approx_sin, "quarter sine")?;
                    cos_code.push(
                        correction_palette
                            .binary_search(&dc)
                            .map_err(|_| "missing cosine correction".to_string())?
                            as u8,
                    );
                    sin_code.push(
                        correction_palette
                            .binary_search(&ds)
                            .map_err(|_| "missing sine correction".to_string())?
                            as u8,
                    );
                }

                LutStorage::QuarterCorrected {
                    quarter_cos,
                    correction_palette,
                    cos_code,
                    sin_code,
                }
            }
        };

        let lut = Self { mode, storage };
        lut.verify_exact_samples()?;
        Ok(lut)
    }

    pub fn sample(&self, index: usize) -> (i64, i64) {
        let index = index & LUT_MASK;
        match &self.storage {
            LutStorage::Full { cos, sin } => (cos[index] as i64, sin[index] as i64),
            LutStorage::CosineCorrected {
                cos,
                sin_correction,
            } => {
                let c = cos[index] as i64;
                let shifted = cos[(index + LUT_SIZE - LUT_QUARTER) & LUT_MASK] as i64;
                (c, shifted + sin_correction[index] as i64)
            }
            LutStorage::QuarterCorrected {
                quarter_cos,
                correction_palette,
                cos_code,
                sin_code,
            } => {
                let c = quarter_cos_approx(quarter_cos, index)
                    + correction_palette[cos_code[index] as usize] as i64;
                let shifted_index = (index + LUT_SIZE - LUT_QUARTER) & LUT_MASK;
                let s = quarter_cos_approx(quarter_cos, shifted_index)
                    + correction_palette[sin_code[index] as usize] as i64;
                (c, s)
            }
        }
    }

    #[inline(always)]
    pub fn sin_cos(&self, angle: u32) -> (i64, i64) {
        let index = (angle >> LUT_SHIFT) as usize;
        let next = (index + 1) & LUT_MASK;
        let fraction = (angle & LUT_FRAC_MASK) as i64;
        let (c0, s0) = self.sample(index);
        let (c1, s1) = self.sample(next);
        (
            c0 + (((c1 - c0) * fraction) >> LUT_SHIFT),
            s0 + (((s1 - s0) * fraction) >> LUT_SHIFT),
        )
    }

    pub fn verify_exact_samples(&self) -> Result<(), String> {
        for index in 0..LUT_SIZE {
            let expected = canonical_sample(index);
            let actual = self.sample(index);
            if actual != (expected.0 as i64, expected.1 as i64) {
                return Err(format!(
                    "{} LUT mismatch at sample {index}",
                    self.mode.name()
                ));
            }
        }
        Ok(())
    }

    pub fn storage_bytes(&self) -> usize {
        match &self.storage {
            LutStorage::Full { cos, sin } => {
                cos.len() * size_of::<i32>() + sin.len() * size_of::<i32>()
            }
            LutStorage::CosineCorrected {
                cos,
                sin_correction,
            } => cos.len() * size_of::<i32>() + sin_correction.len() * size_of::<i16>(),
            LutStorage::QuarterCorrected {
                quarter_cos,
                correction_palette,
                cos_code,
                sin_code,
            } => {
                quarter_cos.len() * size_of::<i32>()
                    + correction_palette.len() * size_of::<i16>()
                    + cos_code.len() * size_of::<u8>()
                    + sin_code.len() * size_of::<u8>()
            }
        }
    }

    pub fn correction_palette_entries(&self) -> usize {
        match &self.storage {
            LutStorage::QuarterCorrected {
                correction_palette,
                ..
            } => correction_palette.len(),
            _ => 0,
        }
    }
}
