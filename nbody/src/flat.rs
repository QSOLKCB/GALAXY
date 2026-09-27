// SPDX-License-Identifier: Apache-2.0
//! Pointer-free Morton/Z-order Barnes-Hut substrate for later GPU execution.
//!
//! BH #1 keeps the recursive f64 quadtree as the correctness oracle. This module
//! freezes a transfer-shaped representation: stable Morton entries plus a flat
//! cell array with explicit child indices and contiguous body ranges.

use super::{
    add_point_mass, direct_accelerations, root_bounds, validate_bodies, Accel, Body, Config,
    ErrorSummary, ForceStats, TreeStats,
};

pub const MORTON_AXIS_BITS: u32 = 16;
pub const MORTON_TREE_DEPTH: usize = MORTON_AXIS_BITS as usize;
pub const NO_CHILD: u32 = u32::MAX;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MortonEntry {
    pub code: u32,
    pub body_index: u32,
}

#[repr(C)]
#[derive(Clone, Debug, PartialEq)]
pub struct FlatCell {
    pub cx: f64,
    pub cy: f64,
    pub half: f64,
    pub mass: f64,
    pub com_x: f64,
    pub com_y: f64,
    pub start: u32,
    pub end: u32,
    pub depth: u16,
    pub children: [u32; 4],
}

impl FlatCell {
    pub fn count(&self) -> usize {
        (self.end - self.start) as usize
    }

    pub fn is_leaf(&self) -> bool {
        self.children.iter().all(|&child| child == NO_CHILD)
    }
}

#[derive(Debug)]
pub struct FlatTree {
    pub entries: Vec<MortonEntry>,
    pub cells: Vec<FlatCell>,
    pub root: u32,
    pub stats: TreeStats,
    pub body_count: usize,
    pub morton_axis_bits: u32,
}

#[derive(Debug)]
pub struct FlatForceResult {
    pub accelerations: Vec<Accel>,
    pub tree: FlatTree,
    pub stats: ForceStats,
}

#[inline]
fn spread16(mut value: u32) -> u32 {
    value &= 0x0000_ffff;
    value = (value | (value << 8)) & 0x00ff_00ff;
    value = (value | (value << 4)) & 0x0f0f_0f0f;
    value = (value | (value << 2)) & 0x3333_3333;
    value = (value | (value << 1)) & 0x5555_5555;
    value
}

#[inline]
pub fn morton2(x: u16, y: u16) -> u32 {
    spread16(x as u32) | (spread16(y as u32) << 1)
}

fn quantize_axis(value: f64, center: f64, half: f64) -> u16 {
    let low = center - half;
    let width = half * 2.0;
    let scaled = ((value - low) / width * 65_536.0).floor();
    scaled.clamp(0.0, 65_535.0) as u16
}

pub fn morton_key_for_point(x: f64, y: f64, cx: f64, cy: f64, half: f64) -> u32 {
    morton2(
        quantize_axis(x, cx, half),
        quantize_axis(y, cy, half),
    )
}

fn child_bounds(cx: f64, cy: f64, half: f64, quadrant: usize) -> (f64, f64, f64) {
    let child_half = half * 0.5;
    (
        cx + if quadrant & 1 != 0 { child_half } else { -child_half },
        cy + if quadrant & 2 != 0 { child_half } else { -child_half },
        child_half,
    )
}

fn placeholder_cell(
    start: usize,
    end: usize,
    cx: f64,
    cy: f64,
    half: f64,
    depth: usize,
) -> FlatCell {
    FlatCell {
        cx,
        cy,
        half,
        mass: 0.0,
        com_x: 0.0,
        com_y: 0.0,
        start: start as u32,
        end: end as u32,
        depth: depth as u16,
        children: [NO_CHILD; 4],
    }
}

fn build_topology(
    entries: &[MortonEntry],
    cells: &mut Vec<FlatCell>,
    start: usize,
    end: usize,
    cx: f64,
    cy: f64,
    half: f64,
    depth: usize,
    bucket: usize,
    stats: &mut TreeStats,
) -> u32 {
    let cell_index = cells.len() as u32;
    cells.push(placeholder_cell(start, end, cx, cy, half, depth));
    stats.node_count += 1;
    stats.max_depth = stats.max_depth.max(depth);

    if end - start <= bucket || depth >= MORTON_TREE_DEPTH {
        stats.leaf_count += 1;
        return cell_index;
    }

    let shift = 30_u32 - 2 * depth as u32;
    let mut cursor = start;
    while cursor < end {
        let quadrant = ((entries[cursor].code >> shift) & 3) as usize;
        let group_start = cursor;
        cursor += 1;
        while cursor < end && ((entries[cursor].code >> shift) & 3) as usize == quadrant {
            cursor += 1;
        }
        let (child_cx, child_cy, child_half) = child_bounds(cx, cy, half, quadrant);
        let child = build_topology(
            entries,
            cells,
            group_start,
            cursor,
            child_cx,
            child_cy,
            child_half,
            depth + 1,
            bucket,
            stats,
        );
        cells[cell_index as usize].children[quadrant] = child;
    }

    cell_index
}

fn fill_aggregates(bodies: &[Body], entries: &[MortonEntry], cells: &mut [FlatCell]) {
    for cell_index in (0..cells.len()).rev() {
        if cells[cell_index].is_leaf() {
            let start = cells[cell_index].start as usize;
            let end = cells[cell_index].end as usize;
            let mut mass = 0.0;
            let mut wx = 0.0;
            let mut wy = 0.0;
            for entry in &entries[start..end] {
                let body = bodies[entry.body_index as usize];
                mass += body.mass;
                wx += body.mass * body.x;
                wy += body.mass * body.y;
            }
            cells[cell_index].mass = mass;
            cells[cell_index].com_x = wx / mass;
            cells[cell_index].com_y = wy / mass;
        } else {
            let children = cells[cell_index].children;
            let mut mass = 0.0;
            let mut wx = 0.0;
            let mut wy = 0.0;
            for child_index in children {
                if child_index == NO_CHILD {
                    continue;
                }
                let child = &cells[child_index as usize];
                mass += child.mass;
                wx += child.mass * child.com_x;
                wy += child.mass * child.com_y;
            }
            cells[cell_index].mass = mass;
            cells[cell_index].com_x = wx / mass;
            cells[cell_index].com_y = wy / mass;
        }
    }
}

pub fn build_flat_tree(bodies: &[Body], config: Config) -> Result<FlatTree, String> {
    validate_bodies(bodies)?;
    config.validate()?;
    if bodies.len() > u32::MAX as usize {
        return Err("flat Barnes-Hut tree requires at most u32::MAX resident bodies".into());
    }

    let (cx, cy, half) = root_bounds(bodies);
    let mut entries = Vec::with_capacity(bodies.len());
    for (index, body) in bodies.iter().enumerate() {
        entries.push(MortonEntry {
            code: morton_key_for_point(body.x, body.y, cx, cy, half),
            body_index: index as u32,
        });
    }

    // sort_by_key is stable: equal Morton keys retain BH #1 resident order.
    entries.sort_by_key(|entry| entry.code);

    let mut cells = Vec::new();
    let mut stats = TreeStats::default();
    let root = build_topology(
        &entries,
        &mut cells,
        0,
        entries.len(),
        cx,
        cy,
        half,
        0,
        config.bucket,
        &mut stats,
    );
    fill_aggregates(bodies, &entries, &mut cells);

    Ok(FlatTree {
        entries,
        cells,
        root,
        stats,
        body_count: bodies.len(),
        morton_axis_bits: MORTON_AXIS_BITS,
    })
}

#[inline]
fn contains(cell: &FlatCell, body: Body) -> bool {
    body.x >= cell.cx - cell.half
        && body.x <= cell.cx + cell.half
        && body.y >= cell.cy - cell.half
        && body.y <= cell.cy + cell.half
}

pub fn flat_acceleration_for(
    target_index: usize,
    bodies: &[Body],
    tree: &FlatTree,
    config: Config,
    stats: &mut ForceStats,
) -> Result<Accel, String> {
    if target_index >= bodies.len() {
        return Err("body index is out of range".into());
    }
    if tree.body_count != bodies.len() || tree.entries.len() != bodies.len() {
        return Err("flat tree body count does not match resident bodies".into());
    }

    let target = bodies[target_index];
    let mut acceleration = Accel::default();
    let mut stack = Vec::with_capacity(tree.stats.max_depth.saturating_mul(3) + 8);
    stack.push(tree.root);

    while let Some(cell_index) = stack.pop() {
        let cell = &tree.cells[cell_index as usize];
        stats.visited_nodes += 1;

        if cell.is_leaf() {
            for entry in &tree.entries[cell.start as usize..cell.end as usize] {
                let other_index = entry.body_index as usize;
                if other_index == target_index {
                    continue;
                }
                let other = bodies[other_index];
                add_point_mass(
                    &mut acceleration,
                    target,
                    other.x,
                    other.y,
                    other.mass,
                    config,
                );
                stats.direct_terms += 1;
            }
            continue;
        }

        let dx = cell.com_x - target.x;
        let dy = cell.com_y - target.y;
        let distance = dx.hypot(dy);
        let width = cell.half * 2.0;
        if !contains(cell, target) && distance > 0.0 && width / distance < config.theta {
            add_point_mass(
                &mut acceleration,
                target,
                cell.com_x,
                cell.com_y,
                cell.mass,
                config,
            );
            stats.approximated_cells += 1;
            continue;
        }

        // Reverse push preserves deterministic 0 -> 3 child visitation order.
        for quadrant in (0..4).rev() {
            let child = cell.children[quadrant];
            if child != NO_CHILD {
                stack.push(child);
            }
        }
    }

    Ok(acceleration)
}

pub fn flat_accelerations_from_tree(
    bodies: &[Body],
    tree: &FlatTree,
    config: Config,
) -> Result<(Vec<Accel>, ForceStats), String> {
    validate_bodies(bodies)?;
    config.validate()?;
    if tree.body_count != bodies.len() {
        return Err("flat tree body count does not match resident bodies".into());
    }

    let mut stats = ForceStats::default();
    let mut accelerations = Vec::with_capacity(bodies.len());
    for index in 0..bodies.len() {
        accelerations.push(flat_acceleration_for(
            index,
            bodies,
            tree,
            config,
            &mut stats,
        )?);
    }
    Ok((accelerations, stats))
}

pub fn flat_barnes_hut_accelerations(
    bodies: &[Body],
    config: Config,
) -> Result<FlatForceResult, String> {
    let tree = build_flat_tree(bodies, config)?;
    let (accelerations, stats) = flat_accelerations_from_tree(bodies, &tree, config)?;
    Ok(FlatForceResult {
        accelerations,
        tree,
        stats,
    })
}

pub fn compare_flat_to_direct(
    bodies: &[Body],
    config: Config,
) -> Result<(FlatForceResult, ErrorSummary), String> {
    let flat = flat_barnes_hut_accelerations(bodies, config)?;
    let direct = direct_accelerations(bodies, config)?;
    let mut sum_sq = 0.0;
    let mut max_relative = 0.0_f64;
    for (approximate, exact) in flat.accelerations.iter().zip(direct) {
        let denominator = exact.ax.hypot(exact.ay).max(1e-30);
        let relative =
            (approximate.ax - exact.ax).hypot(approximate.ay - exact.ay) / denominator;
        sum_sq += relative * relative;
        max_relative = max_relative.max(relative);
    }
    Ok((
        flat,
        ErrorSummary {
            rms_relative: (sum_sq / bodies.len() as f64).sqrt(),
            max_relative,
        },
    ))
}

fn checksum_word(hash: &mut u64, word: u64) {
    *hash ^= word;
    *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    *hash ^= word.rotate_left(23);
}

pub fn flat_topology_checksum(tree: &FlatTree) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    checksum_word(&mut hash, tree.morton_axis_bits as u64);
    checksum_word(&mut hash, tree.body_count as u64);
    for entry in &tree.entries {
        checksum_word(&mut hash, entry.code as u64);
        checksum_word(&mut hash, entry.body_index as u64);
    }
    for cell in &tree.cells {
        checksum_word(&mut hash, cell.start as u64);
        checksum_word(&mut hash, cell.end as u64);
        checksum_word(&mut hash, cell.depth as u64);
        for child in cell.children {
            checksum_word(&mut hash, child as u64);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{barnes_hut_accelerations, make_disc};

    fn relative(a: Accel, b: Accel) -> f64 {
        (a.ax - b.ax).hypot(a.ay - b.ay) / b.ax.hypot(b.ay).max(1e-30)
    }

    #[test]
    fn flat_layout_has_frozen_host_record_sizes() {
        assert_eq!(std::mem::size_of::<MortonEntry>(), 8);
        assert_eq!(std::mem::size_of::<FlatCell>(), 80);
    }

    #[test]
    fn morton_interleave_has_expected_quadrant_order() {
        assert_eq!(morton2(0, 0), 0);
        assert_eq!(morton2(1, 0), 1);
        assert_eq!(morton2(0, 1), 2);
        assert_eq!(morton2(1, 1), 3);
        assert!(morton2(u16::MAX, 0) < morton2(0, u16::MAX));
    }

    #[test]
    fn stable_equal_morton_keys_preserve_resident_order() {
        let body = Body {
            x: 0.0,
            y: 0.0,
            vx: 0.0,
            vy: 0.0,
            mass: 1.0,
        };
        let bodies = vec![body; 8];
        let tree = build_flat_tree(
            &bodies,
            Config {
                bucket: 1,
                max_depth: 48,
                softening_kpc: 0.1,
                ..Config::default()
            },
        )
        .unwrap();
        let order: Vec<u32> = tree.entries.iter().map(|entry| entry.body_index).collect();
        assert_eq!(order, (0_u32..8).collect::<Vec<_>>());
        assert_eq!(tree.stats.max_depth, MORTON_TREE_DEPTH);
    }

    #[test]
    fn flat_tree_preserves_complete_range_and_mass() {
        let bodies = make_disc(256, 303, 5.0e10, 3.0).unwrap();
        let tree = build_flat_tree(&bodies, Config::default()).unwrap();
        let root = &tree.cells[tree.root as usize];
        assert_eq!(root.start, 0);
        assert_eq!(root.end as usize, bodies.len());
        assert!((root.mass - 5.0e10).abs() < 1e-3);
        assert!(tree.stats.node_count > tree.stats.leaf_count);
        assert!(tree.stats.max_depth <= MORTON_TREE_DEPTH);
    }

    #[test]
    fn flat_theta_zero_matches_direct_to_roundoff() {
        let bodies = make_disc(128, 303, 5.0e10, 3.0).unwrap();
        let config = Config {
            theta: 0.0,
            ..Config::default()
        };
        let flat = flat_barnes_hut_accelerations(&bodies, config).unwrap();
        let direct = direct_accelerations(&bodies, config).unwrap();
        let worst = flat
            .accelerations
            .iter()
            .copied()
            .zip(direct)
            .map(|(a, b)| relative(a, b))
            .fold(0.0_f64, f64::max);
        assert!(worst < 2e-12, "worst flat theta=0 mismatch {worst}");
    }

    #[test]
    fn flat_theta_half_stays_inside_bh1_accuracy_gate() {
        let bodies = make_disc(256, 303, 5.0e10, 3.0).unwrap();
        let (flat, error) = compare_flat_to_direct(&bodies, Config::default()).unwrap();
        assert!(error.rms_relative < 0.03, "RMS error {}", error.rms_relative);
        assert!(error.max_relative < 0.25, "max error {}", error.max_relative);
        assert!(flat.stats.approximated_cells > 0);
    }

    #[test]
    fn flat_and_recursive_barnes_hut_agree_on_fixture() {
        let bodies = make_disc(256, 303, 5.0e10, 3.0).unwrap();
        let config = Config::default();
        let recursive = barnes_hut_accelerations(&bodies, config).unwrap();
        let flat = flat_barnes_hut_accelerations(&bodies, config).unwrap();
        let worst = flat
            .accelerations
            .iter()
            .copied()
            .zip(recursive.accelerations)
            .map(|(a, b)| relative(a, b))
            .fold(0.0_f64, f64::max);
        assert!(worst < 1e-9, "flat/recursive relative mismatch {worst}");
    }

    #[test]
    fn topology_checksum_is_repeatable() {
        let bodies = make_disc(512, 919, 5.0e10, 3.0).unwrap();
        let a = build_flat_tree(&bodies, Config::default()).unwrap();
        let b = build_flat_tree(&bodies, Config::default()).unwrap();
        assert_eq!(flat_topology_checksum(&a), flat_topology_checksum(&b));
    }
}
