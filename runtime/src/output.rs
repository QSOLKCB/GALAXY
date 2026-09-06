// SPDX-License-Identifier: Apache-2.0
use crate::{
    config::{Result, Spin},
    reference::{self, Particle, KMS_TO_KPC_MYR},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
};
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}
pub fn snapshot(directory: &Path, s: &Spin, step: u32, points: &[Particle]) -> Result<Value> {
    if points.len() != s.sample_count() as usize
        || points
            .iter()
            .any(|p| p.state.iter().chain(&p.orbit).any(|v| !v.is_finite()))
    {
        return Err("Invalid snapshot values or sample length".into());
    }
    let stem = format!("frame-{step:06}");
    let mut writer = BufWriter::new(File::create(directory.join(format!("{stem}.csv")))?);
    writeln!(
        writer,
        "particle_id,x_kpc,y_kpc,z_kpc,vx_kms,vy_kms,initial_radius_kpc"
    )?;
    let mut max_radius: f64 = 0.0;
    let mut max_lz_drift: f64 = 0.0;
    for (i, p) in points.iter().enumerate() {
        let [x, y, vx, vy] = p.state.map(f64::from);
        let initial_lz = p.orbit[0] as f64 * p.orbit[0] as f64 * p.orbit[3] as f64;
        max_radius = max_radius.max(x.hypot(y));
        max_lz_drift = max_lz_drift.max(((x * vy - y * vx - initial_lz) / initial_lz).abs());
        writeln!(
            writer,
            "{},{x:.9},{y:.9},{:.9},{:.9},{:.9},{:.9}",
            reference::sample_index(i as u32, s.sample_count(), s.particles),
            p.orbit[2],
            vx / KMS_TO_KPC_MYR,
            vy / KMS_TO_KPC_MYR,
            p.orbit[0]
        )?;
    }
    writer.flush()?;
    let size = s.image_size as usize;
    let mut light = vec![0.0_f32; size * size];
    let tilt = s.inclination_deg.to_radians();
    for p in points {
        let x = p.state[0] as f64;
        let y = p.state[1] as f64 * tilt.cos() + p.orbit[2] as f64 * tilt.sin();
        let px = ((x / s.extent_kpc + 1.0) * 0.5 * size as f64) as i64;
        let py = ((y / s.extent_kpc + 1.0) * 0.5 * size as f64) as i64;
        for dy in -1_i64..=1 {
            for dx in -1_i64..=1 {
                let xx = px + dx;
                let yy = py + dy;
                if xx >= 0 && yy >= 0 && xx < size as i64 && yy < size as i64 {
                    light[yy as usize * size + xx as usize] +=
                        if dx == 0 && dy == 0 { 0.7 } else { 0.12 };
                }
            }
        }
    }
    let mut pixels = Vec::with_capacity(size * size * 3);
    for value in light {
        let value = 1.0 - (-value).exp();
        pixels.extend([
            (3.0 + 237.0 * value) as u8,
            (5.0 + 211.0 * value) as u8,
            (9.0 + 166.0 * value) as u8,
        ]);
    }
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(directory.join(format!("{stem}.png")))?),
        s.image_size,
        s.image_size,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png = encoder.write_header()?;
    png.write_image_data(&pixels)?;
    png.finish()?;
    Ok(
        json!({"step":step,"time_myr":step as f64*s.dt_myr,"image":format!("{stem}.png"),"csv":format!("{stem}.csv"),
        "sampled_particles":points.len(),"max_sampled_radius_kpc":max_radius,"max_sampled_relative_lz_drift":max_lz_drift}),
    )
}
pub fn viewer(directory: &Path, frames: &[Value], particles: u32) -> Result<()> {
    let data = serde_json::to_string(frames)?;
    let html = format!(
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>GALAXY native run</title>
<style>body{{margin:0;background:#080a0d;color:#e6e4df;font:15px system-ui;max-width:1100px;padding:24px;margin:auto}}h1{{font-weight:500;letter-spacing:.12em}}img{{display:block;width:min(100%,850px);margin:20px auto}}input{{width:70%;accent-color:#d7b47c}}button{{padding:8px 16px;background:#292219;color:#efcca0;border:1px solid #745b35;cursor:pointer}}p{{color:#a2a6ae}}a{{color:#d7b47c}}</style>
<h1>GALAXY / NATIVE RUN</h1><p>{particles} simulated particles · preview shows the exported sample</p><img id="frame" alt="Galaxy particle positions"><button id="play">Play</button> <input id="step" aria-label="Snapshot" type="range" min="0" max="{}" value="0"><p id="caption"></p><a href="receipt.json">Run report</a> · <a href="job.json">Resolved job</a>
<script>const frames={data};const slider=document.getElementById('step');let timer;function show(){{const f=frames[Number(slider.value)];document.getElementById('frame').src=f.image;document.getElementById('caption').textContent='Step '+f.step+' · '+f.time_myr.toFixed(2)+' Myr · '+f.sampled_particles+' exported particles';}}slider.oninput=show;document.getElementById('play').onclick=function(){{if(timer){{clearInterval(timer);timer=null;this.textContent='Play';}}else{{this.textContent='Pause';timer=setInterval(()=>{{slider.value=(Number(slider.value)+1)%frames.length;show();}},250);}}}};show();</script></html>"#,
        frames.len().saturating_sub(1)
    );
    fs::write(directory.join("viewer.html"), html)?;
    Ok(())
}
pub fn artifacts(directory: &Path) -> Result<Vec<Value>> {
    let mut files = fs::read_dir(directory)?
        .map(|e| e.map(|v| v.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.sort();
    files.into_iter().filter(|p|p.file_name().unwrap()!="receipt.json" && p.is_file()).map(|p| {
        let bytes = fs::read(&p)?; Ok(json!({"file":p.file_name().unwrap().to_string_lossy(),"bytes":bytes.len(),"sha256":sha256(&bytes)}))
    }).collect()
}
