// SPDX-License-Identifier: Apache-2.0
use galaxy_sampler::retro_math as retro;

fn hex16_words(text: &str) -> [u16; 3] {
    let mut parts = text.split(',').map(|value| u16::from_str_radix(value, 16).unwrap());
    [parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap()]
}
fn hex32(text: &str) -> u32 { u32::from_str_radix(text, 16).unwrap() }

fn main() {
    let mut checked = 0_usize;
    for line in include_str!("../../tests/retro-vectors.txt").lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let fields: Vec<_> = line.split('|').collect();
        match fields[0] {
            "elite_jump" => {
                assert_eq!(
                    retro::elite_jump(hex16_words(fields[1]), fields[2].parse().unwrap()),
                    hex16_words(fields[3]),
                    "{line}"
                );
            }
            "sector" => {
                let seed = hex32(fields[1]);
                let sector = fields[2].parse().unwrap();
                assert_eq!(retro::sector_seed(seed, sector), hex16_words(fields[3]), "{line}");
                assert_eq!(retro::sector_salt(seed, sector), hex32(fields[4]), "{line}");
            }
            "cordic" => {
                assert_eq!(
                    retro::sin_cos_q30(hex32(fields[1])),
                    (fields[2].parse().unwrap(), fields[3].parse().unwrap()),
                    "{line}"
                );
            }
            "project" => {
                assert_eq!(
                    retro::project_q16(fields[1].parse().unwrap(), hex32(fields[2])),
                    (fields[3].parse().unwrap(), fields[4].parse().unwrap()),
                    "{line}"
                );
            }
            "roundtrip" => {
                let start = hex32(fields[1]);
                let delta = hex32(fields[2]);
                let ticks = fields[3].parse().unwrap();
                let advanced = retro::bam_advance(start, delta, ticks);
                assert_eq!(advanced, hex32(fields[4]), "{line}");
                assert_eq!(retro::bam_retreat(advanced, delta, ticks), hex32(fields[5]), "{line}");
            }
            other => panic!("unknown retro vector kind: {other}"),
        }
        checked += 1;
    }
    println!("retro-math: {checked} shared golden vectors passed");
}
