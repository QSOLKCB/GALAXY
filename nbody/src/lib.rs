// SPDX-License-Identifier: Apache-2.0
//! Deterministic planar Barnes-Hut reference for GALAXY.

pub mod flat;

pub const G_KPC3_PER_MSUN_MYR2: f64 = 4.498_502_151_575_286e-12;
pub const DEFAULT_THETA: f64 = 0.5;
pub const DEFAULT_SOFTENING_KPC: f64 = 0.05;
pub const DEFAULT_BUCKET: usize = 4;
pub const DEFAULT_MAX_DEPTH: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub mass: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Accel {
    pub ax: f64,
    pub ay: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub theta: f64,
    pub softening_kpc: f64,
    pub g: f64,
    pub bucket: usize,
    pub max_depth: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theta: DEFAULT_THETA,
            softening_kpc: DEFAULT_SOFTENING_KPC,
            g: G_KPC3_PER_MSUN_MYR2,
            bucket: DEFAULT_BUCKET,
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if !self.theta.is_finite() || self.theta < 0.0 {
            return Err("theta must be finite and >= 0".into());
        }
        if !self.softening_kpc.is_finite() || self.softening_kpc < 0.0 {
            return Err("softening_kpc must be finite and >= 0".into());
        }
        if !self.g.is_finite() || self.g <= 0.0 {
            return Err("g must be positive and finite".into());
        }
        if self.bucket == 0 || self.bucket > 1024 {
            return Err("bucket must be in 1..=1024".into());
        }
        if self.max_depth == 0 || self.max_depth > 128 {
            return Err("max_depth must be in 1..=128".into());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct Node {
    pub cx: f64,
    pub cy: f64,
    pub half: f64,
    pub mass: f64,
    pub com_x: f64,
    pub com_y: f64,
    pub count: usize,
    pub depth: usize,
    pub children: [Option<Box<Node>>; 4],
    pub indices: Vec<usize>,
}

impl Node {
    fn is_leaf(&self) -> bool {
        !self.indices.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeStats {
    pub node_count: usize,
    pub leaf_count: usize,
    pub max_depth: usize,
}

#[derive(Debug)]
pub struct Tree {
    pub root: Node,
    pub stats: TreeStats,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForceStats {
    pub visited_nodes: u64,
    pub approximated_cells: u64,
    pub direct_terms: u64,
}

#[derive(Debug)]
pub struct ForceResult {
    pub accelerations: Vec<Accel>,
    pub tree: Tree,
    pub stats: ForceStats,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ErrorSummary {
    pub rms_relative: f64,
    pub max_relative: f64,
}

fn validate_bodies(bodies: &[Body]) -> Result<(), String> {
    if bodies.len() < 2 {
        return Err("Barnes-Hut requires at least two resident bodies".into());
    }
    for (i, b) in bodies.iter().enumerate() {
        if !b.x.is_finite() || !b.y.is_finite() || !b.vx.is_finite() || !b.vy.is_finite() {
            return Err(format!("body {i} contains non-finite state"));
        }
        if !b.mass.is_finite() || b.mass <= 0.0 {
            return Err(format!("body {i} mass must be positive and finite"));
        }
    }
    Ok(())
}

fn root_bounds(bodies: &[Body]) -> (f64, f64, f64) {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for b in bodies {
        min_x = min_x.min(b.x);
        min_y = min_y.min(b.y);
        max_x = max_x.max(b.x);
        max_y = max_y.max(b.y);
    }
    let span = (max_x - min_x).max(max_y - min_y).max(1e-12);
    (
        (min_x + max_x) * 0.5,
        (min_y + max_y) * 0.5,
        span * 0.500_000_000_001 + 1e-12,
    )
}

fn quadrant(body: Body, cx: f64, cy: f64) -> usize {
    (body.x >= cx) as usize | (((body.y >= cy) as usize) << 1)
}

fn child_bounds(cx: f64, cy: f64, half: f64, q: usize) -> (f64, f64, f64) {
    let h = half * 0.5;
    (
        cx + if q & 1 != 0 { h } else { -h },
        cy + if q & 2 != 0 { h } else { -h },
        h,
    )
}

fn aggregate(bodies: &[Body], indices: &[usize]) -> (f64, f64, f64) {
    let mut mass = 0.0;
    let mut wx = 0.0;
    let mut wy = 0.0;
    for &i in indices {
        let b = bodies[i];
        mass += b.mass;
        wx += b.mass * b.x;
        wy += b.mass * b.y;
    }
    (mass, wx / mass, wy / mass)
}

fn build_node(
    bodies: &[Body],
    indices: Vec<usize>,
    cx: f64,
    cy: f64,
    half: f64,
    depth: usize,
    config: Config,
    stats: &mut TreeStats,
) -> Node {
    stats.node_count += 1;
    stats.max_depth = stats.max_depth.max(depth);
    let (mass, com_x, com_y) = aggregate(bodies, &indices);
    let count = indices.len();

    if count <= config.bucket || depth >= config.max_depth || half <= f64::EPSILON * 32.0 {
        stats.leaf_count += 1;
        return Node {
            cx, cy, half, mass, com_x, com_y, count, depth,
            children: std::array::from_fn(|_| None),
            indices,
        };
    }

    let mut groups: [Vec<usize>; 4] = std::array::from_fn(|_| Vec::new());
    for i in indices {
        groups[quadrant(bodies[i], cx, cy)].push(i);
    }
    let mut children: [Option<Box<Node>>; 4] = std::array::from_fn(|_| None);
    for q in 0..4 {
        if groups[q].is_empty() {
            continue;
        }
        let (child_cx, child_cy, child_half) = child_bounds(cx, cy, half, q);
        let group = std::mem::take(&mut groups[q]);
        children[q] = Some(Box::new(build_node(
            bodies, group, child_cx, child_cy, child_half, depth + 1, config, stats,
        )));
    }

    Node {
        cx, cy, half, mass, com_x, com_y, count, depth,
        children,
        indices: Vec::new(),
    }
}

pub fn build_tree(bodies: &[Body], config: Config) -> Result<Tree, String> {
    validate_bodies(bodies)?;
    config.validate()?;
    let (cx, cy, half) = root_bounds(bodies);
    let mut stats = TreeStats::default();
    let root = build_node(
        bodies, (0..bodies.len()).collect(), cx, cy, half, 0, config, &mut stats,
    );
    Ok(Tree { root, stats })
}

fn contains(node: &Node, body: Body) -> bool {
    body.x >= node.cx - node.half
        && body.x <= node.cx + node.half
        && body.y >= node.cy - node.half
        && body.y <= node.cy + node.half
}

fn add_point_mass(acc: &mut Accel, target: Body, x: f64, y: f64, mass: f64, config: Config) {
    let dx = x - target.x;
    let dy = y - target.y;
    let r2 = dx * dx + dy * dy + config.softening_kpc * config.softening_kpc;
    let inv_r = 1.0 / r2.sqrt();
    let scale = config.g * mass * inv_r * inv_r * inv_r;
    acc.ax += dx * scale;
    acc.ay += dy * scale;
}

fn walk_force(
    target_index: usize,
    target: Body,
    bodies: &[Body],
    node: &Node,
    config: Config,
    acc: &mut Accel,
    stats: &mut ForceStats,
) {
    stats.visited_nodes += 1;
    if node.is_leaf() {
        for &other_index in &node.indices {
            if other_index == target_index {
                continue;
            }
            let other = bodies[other_index];
            add_point_mass(acc, target, other.x, other.y, other.mass, config);
            stats.direct_terms += 1;
        }
        return;
    }

    let dx = node.com_x - target.x;
    let dy = node.com_y - target.y;
    let distance = dx.hypot(dy);
    let width = node.half * 2.0;
    if !contains(node, target) && distance > 0.0 && width / distance < config.theta {
        add_point_mass(acc, target, node.com_x, node.com_y, node.mass, config);
        stats.approximated_cells += 1;
        return;
    }
    for child in node.children.iter().flatten() {
        walk_force(target_index, target, bodies, child, config, acc, stats);
    }
}

pub fn acceleration_for(
    index: usize,
    bodies: &[Body],
    tree: &Tree,
    config: Config,
    stats: &mut ForceStats,
) -> Accel {
    let mut acc = Accel::default();
    walk_force(index, bodies[index], bodies, &tree.root, config, &mut acc, stats);
    acc
}

pub fn barnes_hut_accelerations(bodies: &[Body], config: Config) -> Result<ForceResult, String> {
    let tree = build_tree(bodies, config)?;
    let mut stats = ForceStats::default();
    let mut accelerations = Vec::with_capacity(bodies.len());
    for i in 0..bodies.len() {
        accelerations.push(acceleration_for(i, bodies, &tree, config, &mut stats));
    }
    Ok(ForceResult { accelerations, tree, stats })
}

pub fn direct_acceleration_for(index: usize, bodies: &[Body], config: Config) -> Result<Accel, String> {
    validate_bodies(bodies)?;
    config.validate()?;
    if index >= bodies.len() {
        return Err("body index is out of range".into());
    }
    let target = bodies[index];
    let mut acc = Accel::default();
    for (j, other) in bodies.iter().copied().enumerate() {
        if j != index {
            add_point_mass(&mut acc, target, other.x, other.y, other.mass, config);
        }
    }
    Ok(acc)
}

pub fn direct_accelerations(bodies: &[Body], config: Config) -> Result<Vec<Accel>, String> {
    validate_bodies(bodies)?;
    config.validate()?;
    let mut out = vec![Accel::default(); bodies.len()];
    for i in 0..bodies.len() {
        for j in (i + 1)..bodies.len() {
            let a = bodies[i];
            let b = bodies[j];
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let r2 = dx * dx + dy * dy + config.softening_kpc * config.softening_kpc;
            let inv_r = 1.0 / r2.sqrt();
            let common = config.g * inv_r * inv_r * inv_r;
            out[i].ax += dx * common * b.mass;
            out[i].ay += dy * common * b.mass;
            out[j].ax -= dx * common * a.mass;
            out[j].ay -= dy * common * a.mass;
        }
    }
    Ok(out)
}

pub fn compare_to_direct(bodies: &[Body], config: Config) -> Result<(ForceResult, ErrorSummary), String> {
    let approx = barnes_hut_accelerations(bodies, config)?;
    let exact = direct_accelerations(bodies, config)?;
    let mut sum_sq = 0.0;
    let mut max_relative: f64 = 0.0;
    for (a, e) in approx.accelerations.iter().zip(exact) {
        let denominator = e.ax.hypot(e.ay).max(1e-30);
        let relative = (a.ax - e.ax).hypot(a.ay - e.ay) / denominator;
        sum_sq += relative * relative;
        max_relative = max_relative.max(relative);
    }
    Ok((approx, ErrorSummary {
        rms_relative: (sum_sq / bodies.len() as f64).sqrt(),
        max_relative,
    }))
}

pub fn probe_error(
    bodies: &[Body],
    approximate: &[Accel],
    config: Config,
    probes: usize,
) -> Result<ErrorSummary, String> {
    if approximate.len() != bodies.len() {
        return Err("approximate acceleration count does not match bodies".into());
    }
    let probes = probes.clamp(1, bodies.len());
    let mut sum_sq = 0.0;
    let mut max_relative: f64 = 0.0;
    for k in 0..probes {
        let index = k * (bodies.len() - 1) / probes.saturating_sub(1).max(1);
        let exact = direct_acceleration_for(index, bodies, config)?;
        let a = approximate[index];
        let denominator = exact.ax.hypot(exact.ay).max(1e-30);
        let relative = (a.ax - exact.ax).hypot(a.ay - exact.ay) / denominator;
        sum_sq += relative * relative;
        max_relative = max_relative.max(relative);
    }
    Ok(ErrorSummary {
        rms_relative: (sum_sq / probes as f64).sqrt(),
        max_relative,
    })
}

pub fn leapfrog_step_from_acceleration(
    bodies: &mut [Body],
    dt_myr: f64,
    config: Config,
    first_accelerations: &[Accel],
) -> Result<ForceResult, String> {
    if !dt_myr.is_finite() || dt_myr <= 0.0 {
        return Err("dt_myr must be positive and finite".into());
    }
    config.validate()?;
    validate_bodies(bodies)?;
    if first_accelerations.len() != bodies.len() {
        return Err("boundary acceleration count does not match bodies".into());
    }
    for (body, acc) in bodies.iter_mut().zip(first_accelerations) {
        body.vx += 0.5 * dt_myr * acc.ax;
        body.vy += 0.5 * dt_myr * acc.ay;
        body.x += dt_myr * body.vx;
        body.y += dt_myr * body.vy;
    }
    let second = barnes_hut_accelerations(bodies, config)?;
    for (body, acc) in bodies.iter_mut().zip(&second.accelerations) {
        body.vx += 0.5 * dt_myr * acc.ax;
        body.vy += 0.5 * dt_myr * acc.ay;
    }
    Ok(second)
}

pub fn leapfrog_step(bodies: &mut [Body], dt_myr: f64, config: Config) -> Result<ForceResult, String> {
    let first = barnes_hut_accelerations(bodies, config)?;
    leapfrog_step_from_acceleration(bodies, dt_myr, config, &first.accelerations)
}

#[derive(Clone, Copy, Debug)]
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1_u64 << 53) as f64)
    }
}

fn disc_body(
    rng: &mut XorShift64,
    per_body_mass: f64,
    total_mass: f64,
    scale_kpc: f64,
    cx: f64,
    cy: f64,
    cvx: f64,
    cvy: f64,
    direction: f64,
    spin: f64,
    softening_kpc: f64,
) -> Body {
    let u = (1.0 - rng.unit()).max(1e-15);
    let r = scale_kpc * (-u.ln()).sqrt();
    let angle = std::f64::consts::TAU * rng.unit();
    let x = cx + r * angle.cos();
    let y = cy + r * angle.sin();
    let q = (r / scale_kpc).max(0.0);
    let enclosed_fraction = (1.0 - (-q).exp() * (1.0 + q)).clamp(0.0, 1.0);
    let softened_r = (r * r + softening_kpc * softening_kpc).sqrt().max(1e-9);
    let circular = (G_KPC3_PER_MSUN_MYR2 * total_mass * enclosed_fraction / softened_r).sqrt();
    Body {
        x,
        y,
        vx: cvx - direction * spin * circular * angle.sin(),
        vy: cvy + direction * spin * circular * angle.cos(),
        mass: per_body_mass,
    }
}

pub fn make_disc(count: usize, seed: u64, total_mass_msun: f64, scale_kpc: f64) -> Result<Vec<Body>, String> {
    if !(2..=1_000_000).contains(&count) {
        return Err("particle count must be in 2..=1000000".into());
    }
    if !total_mass_msun.is_finite() || total_mass_msun <= 0.0 || !scale_kpc.is_finite() || scale_kpc <= 0.0 {
        return Err("disc mass and scale must be positive and finite".into());
    }
    let mut rng = XorShift64::new(seed);
    let per_body_mass = total_mass_msun / count as f64;
    let mut bodies = Vec::with_capacity(count);
    for _ in 0..count {
        bodies.push(disc_body(
            &mut rng, per_body_mass, total_mass_msun, scale_kpc,
            0.0, 0.0, 0.0, 0.0, 1.0, 0.88, DEFAULT_SOFTENING_KPC,
        ));
    }
    Ok(bodies)
}

pub fn make_collision(count: usize, seed: u64) -> Result<Vec<Body>, String> {
    if !(4..=1_000_000).contains(&count) {
        return Err("collision particle count must be in 4..=1000000".into());
    }
    let left = count / 2;
    let right = count - left;
    let total_mass = 5.0e10;
    let half_mass = total_mass * 0.5;
    let mut rng_a = XorShift64::new(seed);
    let mut rng_b = XorShift64::new(seed.wrapping_add(1));
    let mut bodies = Vec::with_capacity(count);
    for _ in 0..left {
        bodies.push(disc_body(
            &mut rng_a, half_mass / left as f64, half_mass, 3.0,
            -7.5, -1.2, 0.030, 0.010, 1.0, 0.86, DEFAULT_SOFTENING_KPC,
        ));
    }
    for _ in 0..right {
        bodies.push(disc_body(
            &mut rng_b, half_mass / right as f64, half_mass, 2.7,
            7.5, 1.2, -0.030, -0.010, -1.0, 0.86, DEFAULT_SOFTENING_KPC,
        ));
    }
    Ok(bodies)
}

pub fn state_checksum(bodies: &[Body]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for body in bodies {
        for word in [
            body.x.to_bits(), body.y.to_bits(), body.vx.to_bits(),
            body.vy.to_bits(), body.mass.to_bits()
        ] {
            hash ^= word;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            hash ^= word.rotate_left(23);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relative(a: Accel, b: Accel) -> f64 {
        (a.ax - b.ax).hypot(a.ay - b.ay) / b.ax.hypot(b.ay).max(1e-30)
    }

    #[test]
    fn tree_preserves_count_and_mass() {
        let bodies = make_disc(256, 303, 5.0e10, 3.0).unwrap();
        let tree = build_tree(&bodies, Config::default()).unwrap();
        assert_eq!(tree.root.count, bodies.len());
        assert!((tree.root.mass - 5.0e10).abs() < 1e-3);
        assert!(tree.stats.node_count > tree.stats.leaf_count);
        assert!(tree.stats.max_depth > 0);
    }

    #[test]
    fn theta_zero_matches_direct_to_roundoff() {
        let bodies = make_disc(128, 303, 5.0e10, 3.0).unwrap();
        let config = Config { theta: 0.0, ..Config::default() };
        let tree = barnes_hut_accelerations(&bodies, config).unwrap();
        let direct = direct_accelerations(&bodies, config).unwrap();
        let worst = tree.accelerations.iter().copied().zip(direct)
            .map(|(a, b)| relative(a, b)).fold(0.0_f64, f64::max);
        assert!(worst < 2e-12, "worst theta=0 relative mismatch {worst}");
    }

    #[test]
    fn theta_half_is_bounded_against_direct_reference() {
        let bodies = make_disc(256, 303, 5.0e10, 3.0).unwrap();
        let (result, error) = compare_to_direct(&bodies, Config::default()).unwrap();
        assert!(error.rms_relative < 0.03, "RMS error {}", error.rms_relative);
        assert!(error.max_relative < 0.25, "max error {}", error.max_relative);
        assert!(result.stats.approximated_cells > 0);
    }

    #[test]
    fn duplicate_positions_terminate_at_depth_bound() {
        let body = Body { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, mass: 1.0 };
        let bodies = vec![body; 8];
        let config = Config { bucket: 1, max_depth: 12, softening_kpc: 0.1, ..Config::default() };
        let tree = build_tree(&bodies, config).unwrap();
        assert!(tree.stats.max_depth <= 12);
        let result = barnes_hut_accelerations(&bodies, config).unwrap();
        assert!(result.accelerations.iter().all(|a| a.ax.is_finite() && a.ay.is_finite()));
    }

    #[test]
    fn leapfrog_moves_finite_state() {
        let mut bodies = make_disc(128, 77, 5.0e10, 3.0).unwrap();
        let before = bodies.clone();
        leapfrog_step(&mut bodies, 0.01, Config::default()).unwrap();
        assert!(bodies.iter().zip(before).any(|(a, b)| a.x != b.x || a.y != b.y));
        assert!(bodies.iter().all(|b| [b.x, b.y, b.vx, b.vy].into_iter().all(f64::is_finite)));
    }

    #[test]
    fn reused_boundary_acceleration_matches_convenience_step() {
        let config = Config::default();
        let mut convenience = make_disc(128, 88, 5.0e10, 3.0).unwrap();
        let mut reused = convenience.clone();
        let first = barnes_hut_accelerations(&reused, config).unwrap();

        let convenience_result = leapfrog_step(&mut convenience, 0.01, config).unwrap();
        let reused_result =
            leapfrog_step_from_acceleration(&mut reused, 0.01, config, &first.accelerations).unwrap();

        assert_eq!(convenience, reused);
        assert_eq!(convenience_result.accelerations, reused_result.accelerations);
        assert_eq!(convenience_result.stats, reused_result.stats);
        assert_eq!(convenience_result.tree.stats, reused_result.tree.stats);
    }

    #[test]
    fn reused_boundary_acceleration_rejects_wrong_length() {
        let config = Config::default();
        let mut bodies = make_disc(8, 89, 5.0e10, 3.0).unwrap();
        let error = leapfrog_step_from_acceleration(&mut bodies, 0.01, config, &[]).unwrap_err();
        assert!(error.contains("acceleration count"));
    }

    #[test]
    fn seeded_state_and_checksum_are_repeatable() {
        let a = make_collision(256, 999).unwrap();
        let b = make_collision(256, 999).unwrap();
        assert_eq!(a, b);
        assert_eq!(state_checksum(&a), state_checksum(&b));
    }
}
