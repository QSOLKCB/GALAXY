// SPDX-License-Identifier: Apache-2.0
use galaxy_nbody::flat::{
    build_flat_tree, compare_flat_to_direct, flat_accelerations_from_tree, flat_topology_checksum,
    FlatCell, MortonEntry, MORTON_AXIS_BITS,
};
use galaxy_nbody::{
    barnes_hut_accelerations, compare_to_direct, leapfrog_step_from_acceleration, make_collision,
    make_disc, probe_error, state_checksum, Accel, Body, Config, DEFAULT_SOFTENING_KPC,
};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAX_PARTICLES: usize = 1_000_000;
const MAX_STEPS: usize = 1_000_000;

fn usage() -> &'static str {
    "GALAXY Barnes-Hut self-gravity reference\n\n\
usage:\n\
  galaxy-nbody verify\n\
  galaxy-nbody verify-flat\n\
  galaxy-nbody flat-probe [--preset disc|collision] [--particles N] [--theta F]\n\
                          [--softening-kpc F] [--seed N] [--receipt PATH]\n\
  galaxy-nbody run [--preset disc|collision] [--particles N] [--steps N]\n\
                   [--theta F] [--dt-myr F] [--softening-kpc F] [--seed N]\n\
                   [--receipt PATH] [--snapshot PATH]\n\n\
The run path is resident and mutually coupled: it does not accept GALAXY logical-u64\n\
population tiling because Barnes-Hut self-gravity has cross-particle forces.\n"
}

fn parse_args(args: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let key = &args[i];
        if !key.starts_with("--") {
            return Err(format!("unexpected positional argument: {key}"));
        }
        let value = args.get(i + 1).ok_or_else(|| format!("missing value for {key}"))?;
        if value.starts_with("--") {
            return Err(format!("missing value for {key}"));
        }
        if out.insert(key.clone(), value.clone()).is_some() {
            return Err(format!("duplicate option: {key}"));
        }
        i += 2;
    }
    Ok(out)
}

fn usize_arg(
    map: &BTreeMap<String, String>,
    key: &str,
    default: usize,
    max: usize,
) -> Result<usize, String> {
    let Some(text) = map.get(key) else { return Ok(default); };
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{key} must be an unsigned decimal integer"));
    }
    let value = text.parse::<usize>().map_err(|_| format!("{key} is out of range"))?;
    if value == 0 || value > max {
        return Err(format!("{key} must be in 1..={max}"));
    }
    Ok(value)
}

fn u64_arg(map: &BTreeMap<String, String>, key: &str, default: u64) -> Result<u64, String> {
    let Some(text) = map.get(key) else { return Ok(default); };
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{key} must be an unsigned decimal integer"));
    }
    text.parse::<u64>().map_err(|_| format!("{key} is out of range"))
}

fn f64_arg(
    map: &BTreeMap<String, String>,
    key: &str,
    default: f64,
    min: f64,
    max: f64,
) -> Result<f64, String> {
    let Some(text) = map.get(key) else { return Ok(default); };
    let value = text.parse::<f64>().map_err(|_| format!("{key} must be a decimal number"))?;
    if !value.is_finite() || value < min || value > max {
        return Err(format!("{key} must be finite and in [{min}, {max}]"));
    }
    Ok(value)
}

fn json_escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) else {
        return Ok(());
    };
    fs::create_dir_all(parent)
        .map_err(|e| format!("could not create output directory {}: {e}", parent.display()))
}

fn write_snapshot(path: &PathBuf, bodies: &[galaxy_nbody::Body]) -> Result<(), String> {
    let mut csv = String::from("index,x_kpc,y_kpc,vx_kpc_per_myr,vy_kpc_per_myr,mass_msun\n");
    for (i, b) in bodies.iter().enumerate() {
        csv.push_str(&format!(
            "{i},{:.17},{:.17},{:.17},{:.17},{:.17}\n",
            b.x, b.y, b.vx, b.vy, b.mass
        ));
    }
    fs::write(path, csv).map_err(|e| format!("could not write snapshot {}: {e}", path.display()))
}

fn verify() -> Result<(), String> {
    let bodies = make_disc(256, 303, 5.0e10, 3.0)?;
    let (result, error) = compare_to_direct(&bodies, Config::default())?;
    let theta_zero = Config { theta: 0.0, ..Config::default() };
    let (_, zero_error) = compare_to_direct(&bodies, theta_zero)?;
    if error.rms_relative >= 0.03 || error.max_relative >= 0.25 {
        return Err(format!(
            "theta=0.5 reference error too large: rms={} max={}",
            error.rms_relative, error.max_relative
        ));
    }
    if zero_error.max_relative >= 2e-12 {
        return Err(format!(
            "theta=0 traversal diverged from direct reference: max={}",
            zero_error.max_relative
        ));
    }
    println!(
        "Barnes-Hut verify PASS: bodies={} nodes={} leaves={} depth={} theta0.5_rms={:.8} theta0.5_max={:.8} theta0_max={:.3e}",
        bodies.len(),
        result.tree.stats.node_count,
        result.tree.stats.leaf_count,
        result.tree.stats.max_depth,
        error.rms_relative,
        error.max_relative,
        zero_error.max_relative
    );
    Ok(())
}

fn relative_difference(a: Accel, b: Accel) -> f64 {
    (a.ax - b.ax).hypot(a.ay - b.ay) / b.ax.hypot(b.ay).max(1e-30)
}

fn verify_flat() -> Result<(), String> {
    if std::mem::size_of::<MortonEntry>() != 8 || std::mem::size_of::<FlatCell>() != 80 {
        return Err(format!(
            "flat host record sizes changed: MortonEntry={} FlatCell={}",
            std::mem::size_of::<MortonEntry>(),
            std::mem::size_of::<FlatCell>()
        ));
    }

    let coincident = vec![
        Body { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, mass: 1.0 };
        8
    ];
    let ordering_tree = build_flat_tree(
        &coincident,
        Config {
            bucket: 1,
            max_depth: MORTON_AXIS_BITS as usize,
            softening_kpc: 0.1,
            ..Config::default()
        },
    )?;
    let ordering: Vec<u32> = ordering_tree
        .entries
        .iter()
        .map(|entry| entry.body_index)
        .collect();
    if ordering != (0_u32..8).collect::<Vec<_>>() {
        return Err("equal Morton keys no longer preserve resident order".into());
    }

    let shallow_tree = build_flat_tree(
        &coincident,
        Config {
            bucket: 1,
            max_depth: 1,
            softening_kpc: 0.1,
            ..Config::default()
        },
    )?;
    if shallow_tree.stats.max_depth != 1 {
        return Err(format!(
            "configured max_depth=1 was not honored: observed {}",
            shallow_tree.stats.max_depth
        ));
    }

    let bodies = make_disc(256, 303, 5.0e10, 3.0)?;
    let config = Config::default();
    let (flat, error) = compare_flat_to_direct(&bodies, config)?;
    let root = &flat.tree.cells[flat.tree.root as usize];
    if root.start != 0 || root.end as usize != bodies.len() {
        return Err(format!(
            "flat root range is invalid: [{}..{}) for {} bodies",
            root.start,
            root.end,
            bodies.len()
        ));
    }
    if (root.mass - 5.0e10).abs() >= 1e-3 {
        return Err(format!("flat root mass is invalid: {}", root.mass));
    }
    if error.rms_relative >= 0.03 || error.max_relative >= 0.25 {
        return Err(format!(
            "flat theta=0.5 reference error too large: rms={} max={}",
            error.rms_relative, error.max_relative
        ));
    }

    let recursive = barnes_hut_accelerations(&bodies, config)?;
    let flat_recursive_max = flat
        .accelerations
        .iter()
        .copied()
        .zip(recursive.accelerations.iter().copied())
        .map(|(a, b)| relative_difference(a, b))
        .fold(0.0_f64, f64::max);
    if flat_recursive_max >= 1e-9 {
        return Err(format!(
            "flat Morton traversal diverged from BH #1 recursive oracle: max={flat_recursive_max}"
        ));
    }

    let theta_zero = Config { theta: 0.0, ..config };
    let (_, zero_error) = compare_flat_to_direct(&bodies, theta_zero)?;
    if zero_error.max_relative >= 2e-12 {
        return Err(format!(
            "flat theta=0 traversal diverged from direct reference: max={}",
            zero_error.max_relative
        ));
    }

    let topology_checksum = flat_topology_checksum(&flat.tree);
    let repeat = build_flat_tree(&bodies, config)?;
    if topology_checksum != flat_topology_checksum(&repeat) {
        return Err("flat Morton topology checksum is not repeatable".into());
    }

    println!(
        "Barnes-Hut flat verify PASS: bodies={} cells={} leaves={} depth={} morton_bits={} topology={:016x} theta0.5_rms={:.8} theta0.5_max={:.8} flat_recursive_max={:.3e} theta0_max={:.3e}",
        bodies.len(),
        flat.tree.cells.len(),
        flat.tree.stats.leaf_count,
        flat.tree.stats.max_depth,
        MORTON_AXIS_BITS,
        topology_checksum,
        error.rms_relative,
        error.max_relative,
        flat_recursive_max,
        zero_error.max_relative
    );
    Ok(())
}

fn flat_probe(args: &[String]) -> Result<(), String> {
    let map = parse_args(args)?;
    for key in map.keys() {
        match key.as_str() {
            "--preset" | "--particles" | "--theta" | "--softening-kpc" | "--seed"
            | "--receipt" => {}
            _ => return Err(format!("unknown option for flat-probe: {key}")),
        }
    }

    let preset = map.get("--preset").map(String::as_str).unwrap_or("collision");
    if preset != "disc" && preset != "collision" {
        return Err("--preset must be disc or collision".into());
    }
    let particles = usize_arg(&map, "--particles", 16_384, MAX_PARTICLES)?;
    if particles < 2 {
        return Err("--particles must be at least 2".into());
    }
    if preset == "collision" && particles < 4 {
        return Err("collision preset requires at least 4 particles".into());
    }
    let theta = f64_arg(&map, "--theta", 0.5, 0.0, 2.0)?;
    let softening_kpc = f64_arg(
        &map,
        "--softening-kpc",
        DEFAULT_SOFTENING_KPC,
        0.0,
        100.0,
    )?;
    let seed = u64_arg(&map, "--seed", 303)?;
    let config = Config { theta, softening_kpc, ..Config::default() };
    config.validate()?;

    let bodies = if preset == "collision" {
        make_collision(particles, seed)?
    } else {
        make_disc(particles, seed, 5.0e10, 3.0)?
    };

    let receipt_path = map.get("--receipt").map(PathBuf::from);
    if let Some(path) = receipt_path.as_deref() {
        ensure_parent(path)?;
    }

    let build_started = Instant::now();
    let tree = build_flat_tree(&bodies, config)?;
    let build_elapsed = build_started.elapsed();

    let traversal_started = Instant::now();
    let (accelerations, stats) = flat_accelerations_from_tree(&bodies, &tree, config)?;
    let traversal_elapsed = traversal_started.elapsed();

    let probe_count = 12.min(particles);
    let probe_started = Instant::now();
    let probe = probe_error(&bodies, &accelerations, config, probe_count)?;
    let probe_elapsed = probe_started.elapsed();

    let topology_checksum = flat_topology_checksum(&tree);
    let max_leaf_bodies = tree
        .cells
        .iter()
        .filter(|cell| cell.is_leaf())
        .map(|cell| cell.count())
        .max()
        .unwrap_or(0);
    let layout_bytes = (tree.entries.len() as u128)
        * (std::mem::size_of::<MortonEntry>() as u128)
        + (tree.cells.len() as u128) * (std::mem::size_of::<FlatCell>() as u128);
    let force_terms = stats.direct_terms + stats.approximated_cells;
    let exact_terms = (particles as u128) * ((particles - 1) as u128);
    let avoided_fraction = if exact_terms == 0 {
        0.0
    } else {
        1.0 - force_terms as f64 / exact_terms as f64
    };

    let receipt = format!(
        concat!(
            "{{\n",
            "  \"schema\": \"galaxy.barnes-hut-flat-receipt.v1\",\n",
            "  \"status\": \"complete\",\n",
            "  \"backend\": \"cpu-flat-reference\",\n",
            "  \"preset\": \"{}\",\n",
            "  \"particles\": {},\n",
            "  \"theta\": {:.17},\n",
            "  \"softening_kpc\": {:.17},\n",
            "  \"seed\": {},\n",
            "  \"morton_axis_bits\": {},\n",
            "  \"morton_entry_bytes\": {},\n",
            "  \"flat_cell_bytes\": {},\n",
            "  \"flat_layout_bytes\": {},\n",
            "  \"tree_cells\": {},\n",
            "  \"tree_leaves\": {},\n",
            "  \"tree_max_depth\": {},\n",
            "  \"max_leaf_bodies\": {},\n",
            "  \"topology_checksum_fnv_mix64\": \"{:016x}\",\n",
            "  \"build_seconds\": {:.9},\n",
            "  \"traversal_seconds\": {:.9},\n",
            "  \"direct_probe_seconds\": {:.9},\n",
            "  \"visited_nodes\": {},\n",
            "  \"direct_terms\": {},\n",
            "  \"approximated_cells\": {},\n",
            "  \"force_term_avoided_fraction_vs_direct\": {:.12},\n",
            "  \"direct_probe_count\": {},\n",
            "  \"direct_probe_rms_relative_error\": {:.12},\n",
            "  \"direct_probe_max_relative_error\": {:.12},\n",
            "  \"scope\": \"CPU Morton/flat-tree substrate only; no GPU execution or GPU speed claim; build and traversal timings are reported separately\"\n",
            "}}\n"
        ),
        json_escape(preset),
        particles,
        theta,
        softening_kpc,
        seed,
        MORTON_AXIS_BITS,
        std::mem::size_of::<MortonEntry>(),
        std::mem::size_of::<FlatCell>(),
        layout_bytes,
        tree.cells.len(),
        tree.stats.leaf_count,
        tree.stats.max_depth,
        max_leaf_bodies,
        topology_checksum,
        build_elapsed.as_secs_f64(),
        traversal_elapsed.as_secs_f64(),
        probe_elapsed.as_secs_f64(),
        stats.visited_nodes,
        stats.direct_terms,
        stats.approximated_cells,
        avoided_fraction,
        probe_count,
        probe.rms_relative,
        probe.max_relative
    );

    if let Some(path) = receipt_path.as_ref() {
        fs::write(path, &receipt)
            .map_err(|e| format!("could not write flat receipt {}: {e}", path.display()))?;
    }
    print!("{receipt}");
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    let map = parse_args(args)?;
    for key in map.keys() {
        match key.as_str() {
            "--preset" | "--particles" | "--steps" | "--theta" | "--dt-myr"
            | "--softening-kpc" | "--seed" | "--receipt" | "--snapshot" => {}
            _ => return Err(format!("unknown option: {key}")),
        }
    }

    let preset = map.get("--preset").map(String::as_str).unwrap_or("collision");
    if preset != "disc" && preset != "collision" {
        return Err("--preset must be disc or collision".into());
    }
    let particles = usize_arg(&map, "--particles", 4096, MAX_PARTICLES)?;
    if particles < 2 {
        return Err("--particles must be at least 2".into());
    }
    let steps = usize_arg(&map, "--steps", 64, MAX_STEPS)?;
    let theta = f64_arg(&map, "--theta", 0.5, 0.0, 2.0)?;
    let dt_myr = f64_arg(&map, "--dt-myr", 0.05, 1e-8, 100.0)?;
    let softening_kpc = f64_arg(
        &map,
        "--softening-kpc",
        DEFAULT_SOFTENING_KPC,
        0.0,
        100.0,
    )?;
    let seed = u64_arg(&map, "--seed", 303)?;
    let config = Config { theta, softening_kpc, ..Config::default() };
    config.validate()?;

    let mut bodies = if preset == "collision" {
        if particles < 4 {
            return Err("collision preset requires at least 4 particles".into());
        }
        make_collision(particles, seed)?
    } else {
        make_disc(particles, seed, 5.0e10, 3.0)?
    };

    let snapshot_path = map.get("--snapshot").map(PathBuf::from);
    let receipt_path = map.get("--receipt").map(PathBuf::from);
    if let Some(path) = snapshot_path.as_deref() {
        ensure_parent(path)?;
    }
    if let Some(path) = receipt_path.as_deref() {
        ensure_parent(path)?;
    }

    let started = Instant::now();
    let mut latest = barnes_hut_accelerations(&bodies, config)?;
    for _ in 0..steps {
        let next =
            leapfrog_step_from_acceleration(&mut bodies, dt_myr, config, &latest.accelerations)?;
        latest = next;
    }
    let elapsed = started.elapsed();
    let probe_count = 12.min(particles);
    let probe = probe_error(&bodies, &latest.accelerations, config, probe_count)?;
    let checksum = state_checksum(&bodies);
    let force_terms = latest.stats.direct_terms + latest.stats.approximated_cells;
    let exact_terms = (particles as u128) * ((particles - 1) as u128);
    let avoided_fraction = if exact_terms == 0 {
        0.0
    } else {
        1.0 - (force_terms as f64 / exact_terms as f64)
    };

    if let Some(path) = snapshot_path.as_ref() {
        write_snapshot(path, &bodies)?;
    }

    let receipt = format!(
        concat!(
            "{{\n",
            "  \"schema\": \"galaxy.barnes-hut-receipt.v1\",\n",
            "  \"status\": \"complete\",\n",
            "  \"preset\": \"{}\",\n",
            "  \"particles\": {},\n",
            "  \"steps\": {},\n",
            "  \"theta\": {:.17},\n",
            "  \"dt_myr\": {:.17},\n",
            "  \"softening_kpc\": {:.17},\n",
            "  \"seed\": {},\n",
            "  \"elapsed_seconds\": {:.9},\n",
            "  \"tree_nodes\": {},\n",
            "  \"tree_leaves\": {},\n",
            "  \"tree_max_depth\": {},\n",
            "  \"visited_nodes\": {},\n",
            "  \"direct_terms\": {},\n",
            "  \"approximated_cells\": {},\n",
            "  \"force_term_avoided_fraction_vs_direct\": {:.12},\n",
            "  \"direct_probe_count\": {},\n",
            "  \"direct_probe_rms_relative_error\": {:.12},\n",
            "  \"direct_probe_max_relative_error\": {:.12},\n",
            "  \"state_checksum_fnv_mix64\": \"{:016x}\",\n",
            "  \"scope\": \"resident planar self-gravity; no logical-u64 tiling; f64 results are not claimed bit-identical across architectures\"\n",
            "}}\n"
        ),
        json_escape(preset),
        particles,
        steps,
        theta,
        dt_myr,
        softening_kpc,
        seed,
        elapsed.as_secs_f64(),
        latest.tree.stats.node_count,
        latest.tree.stats.leaf_count,
        latest.tree.stats.max_depth,
        latest.stats.visited_nodes,
        latest.stats.direct_terms,
        latest.stats.approximated_cells,
        avoided_fraction,
        probe_count,
        probe.rms_relative,
        probe.max_relative,
        checksum
    );

    if let Some(path) = receipt_path.as_ref() {
        fs::write(path, &receipt)
            .map_err(|e| format!("could not write receipt {}: {e}", path.display()))?;
    }
    print!("{receipt}");
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("verify") if args.len() == 1 => verify(),
        Some("verify-flat") if args.len() == 1 => verify_flat(),
        Some("flat-probe") => flat_probe(&args[1..]),
        Some("run") => run(&args[1..]),
        Some("--help") | Some("-h") | Some("help") | None => {
            print!("{}", usage());
            return;
        }
        Some(other) => Err(format!("unknown command: {other}\n\n{}", usage())),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(2);
    }
}
