// SPDX-License-Identifier: Apache-2.0
(function () {
  "use strict";
  const BH = globalThis.GalaxyBarnesHut;
  const byId = id => document.getElementById(id);
  const canvas = byId("nbodyCanvas");
  const ctx = canvas.getContext("2d", { alpha: false });
  if (!BH || !ctx) throw new Error("Barnes-Hut laboratory could not start");

  const state = {
    bodies: [], latest: null, running: true, step: 0, lastFrame: 0, fpsStart: performance.now(), frames: 0,
    preset: "collision", count: 1024, seed: 303, theta: 0.5, softening: 0.03, bucket: 4,
    dt: 0.004, stepsPerFrame: 1, showTree: true, treeDepth: 5, probeRms: null
  };

  function options() { return { theta: state.theta, softening: state.softening, bucket: state.bucket, maxDepth: 40, G: 1 }; }
  function formatInt(n) { return Math.round(n).toLocaleString("en-US"); }
  function presetName() { return state.preset === "collision" ? "Binary disc encounter" : state.preset === "cold" ? "Cold collapse" : "Single rotating disc"; }

  function reset() {
    if (state.preset === "collision") state.bodies = BH.makeCollision(state.count, state.seed);
    else {
      state.bodies = BH.makeDisc(state.count, state.seed, { radius: state.preset === "cold" ? 1.05 : 0.95, spin: state.preset === "cold" ? 0.05 : 0.72 });
    }
    state.latest = BH.accelerations(state.bodies, options());
    state.step = 0;
    invalidateAudit();
    byId("presetTitle").textContent = presetName();
    updateReadouts();
  }

  function resize() {
    const rect = canvas.getBoundingClientRect();
    const dpr = Math.min(globalThis.devicePixelRatio || 1, 2);
    const width = Math.max(1, Math.round(rect.width * dpr));
    const height = Math.max(1, Math.round(rect.height * dpr));
    if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height; }
  }

  function extent() {
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const b of state.bodies) {
      minX = Math.min(minX, b.x); minY = Math.min(minY, b.y); maxX = Math.max(maxX, b.x); maxY = Math.max(maxY, b.y);
    }
    const cx = (minX + maxX) * 0.5, cy = (minY + maxY) * 0.5;
    const span = Math.max(maxX - minX, maxY - minY, 2.4) * 1.15;
    return { cx, cy, span };
  }

  function draw() {
    resize();
    const w = canvas.width, h = canvas.height, view = extent();
    const scale = Math.min(w, h) / view.span;
    const sx = x => w * 0.5 + (x - view.cx) * scale;
    const sy = y => h * 0.5 - (y - view.cy) * scale;
    ctx.fillStyle = "#020304"; ctx.fillRect(0, 0, w, h);

    if (state.showTree && state.latest) {
      ctx.lineWidth = Math.max(1, Math.min(globalThis.devicePixelRatio || 1, 2) * 0.6);
      for (const node of BH.flattenTree(state.latest.tree, state.treeDepth)) {
        if (node.depth === 0) continue;
        const alpha = Math.max(0.05, 0.32 - node.depth * 0.035);
        ctx.strokeStyle = `rgba(214,173,103,${alpha})`;
        const left = sx(node.cx - node.half), right = sx(node.cx + node.half);
        const top = sy(node.cy + node.half), bottom = sy(node.cy - node.half);
        ctx.strokeRect(left, top, right - left, bottom - top);
      }
    }

    ctx.globalCompositeOperation = "lighter";
    const r = Math.max(0.65, Math.min(1.8, 20 / Math.sqrt(state.bodies.length))) * (globalThis.devicePixelRatio || 1);
    for (let i = 0; i < state.bodies.length; i++) {
      const b = state.bodies[i];
      const speed = Math.hypot(b.vx, b.vy);
      const light = Math.min(1, 0.45 + speed * 0.65);
      ctx.globalAlpha = light;
      ctx.fillStyle = i & 1 ? "#dad6cb" : "#b8bdc3";
      ctx.beginPath(); ctx.arc(sx(b.x), sy(b.y), r, 0, Math.PI * 2); ctx.fill();
    }
    ctx.globalAlpha = 1; ctx.globalCompositeOperation = "source-over";
  }

  function updateReadouts() {
    const n = state.bodies.length;
    byId("bodyReadout").textContent = formatInt(n);
    byId("thetaBadge").textContent = "θ " + state.theta.toFixed(2);
    byId("timeReadout").textContent = "STEP " + formatInt(state.step);
    if (state.latest) {
      const stats = state.latest.stats;
      const terms = stats.direct + stats.approximated;
      const exactTerms = n * (n - 1);
      const reduction = exactTerms ? Math.max(0, 1 - terms / exactTerms) : 0;
      byId("nodeReadout").textContent = formatInt(state.latest.tree.nodeCount);
      byId("depthReadout").textContent = state.latest.tree.maxDepth + " levels · " + formatInt(state.latest.tree.leafCount) + " leaves";
      byId("termReadout").textContent = formatInt(terms);
      byId("reductionReadout").textContent = (reduction * 100).toFixed(1) + "% fewer force terms than direct";
    }
    byId("errorReadout").textContent = state.probeRms === null ? "—" : (state.probeRms * 100).toFixed(2) + "%";
    byId("runBadge").textContent = state.running ? "RUNNING" : "PAUSED";
    byId("playToggle").textContent = state.running ? "Pause" : "Resume";
  }

  function invalidateAudit(message = "Run the direct-force probe for the current state.") {
    state.probeRms = null;
    byId("auditStatus").textContent = message;
  }

  function integrateOnce() {
    state.latest = BH.stepLeapfrog(state.bodies, state.dt, options());
    state.step++;
    invalidateAudit("State advanced; run the direct-force probe again for this step.");
  }

  function audit() {
    if (!state.latest) return;
    const n = state.bodies.length;
    const probes = Math.min(12, n);
    let sumSq = 0, max = 0;
    for (let k = 0; k < probes; k++) {
      const index = Math.floor(k * (n - 1) / Math.max(1, probes - 1));
      const exact = BH.directAccelerationFor(index, state.bodies, options());
      const approx = state.latest.values[index];
      const denom = Math.max(Math.hypot(exact.ax, exact.ay), 1e-30);
      const rel = Math.hypot(approx.ax - exact.ax, approx.ay - exact.ay) / denom;
      sumSq += rel * rel; max = Math.max(max, rel);
    }
    state.probeRms = Math.sqrt(sumSq / probes);
    byId("auditStatus").textContent = `Direct audit: ${probes} probes · RMS ${(state.probeRms * 100).toFixed(3)}% · max ${(max * 100).toFixed(3)}%.`;
    updateReadouts();
  }

  function syncControlLabels() {
    byId("countValue").textContent = state.count;
    byId("thetaValue").textContent = state.theta.toFixed(2);
    byId("softeningValue").textContent = state.softening.toFixed(3);
    byId("bucketValue").textContent = state.bucket;
    byId("treeDepthValue").textContent = state.treeDepth;
    byId("dtValue").textContent = state.dt.toFixed(4);
    byId("stepsValue").textContent = state.stepsPerFrame;
  }

  function bind() {
    byId("playToggle").addEventListener("click", () => { state.running = !state.running; updateReadouts(); });
    byId("singleStep").addEventListener("click", () => { integrateOnce(); draw(); updateReadouts(); });
    byId("reset").addEventListener("click", reset);
    byId("audit").addEventListener("click", audit);
    byId("preset").addEventListener("change", e => { state.preset = e.target.value; reset(); });
    byId("count").addEventListener("input", e => { state.count = Number(e.target.value); syncControlLabels(); });
    byId("count").addEventListener("change", reset);
    byId("seed").addEventListener("change", e => { state.seed = Math.max(1, Math.trunc(Number(e.target.value) || 303)); reset(); });
    byId("theta").addEventListener("input", e => { state.theta = Number(e.target.value); syncControlLabels(); state.latest = BH.accelerations(state.bodies, options()); invalidateAudit("Opening angle changed; run the direct-force probe again."); updateReadouts(); });
    byId("softening").addEventListener("input", e => { state.softening = Number(e.target.value); syncControlLabels(); state.latest = BH.accelerations(state.bodies, options()); invalidateAudit("Softening changed; run the direct-force probe again."); updateReadouts(); });
    byId("bucket").addEventListener("input", e => { state.bucket = Number(e.target.value); syncControlLabels(); state.latest = BH.accelerations(state.bodies, options()); invalidateAudit("Leaf bucket changed; run the direct-force probe again."); updateReadouts(); });
    byId("showTree").addEventListener("change", e => { state.showTree = e.target.checked; draw(); });
    byId("treeDepth").addEventListener("input", e => { state.treeDepth = Number(e.target.value); syncControlLabels(); draw(); });
    byId("dt").addEventListener("input", e => { state.dt = Number(e.target.value); syncControlLabels(); });
    byId("steps").addEventListener("input", e => { state.stepsPerFrame = Number(e.target.value); syncControlLabels(); });
    globalThis.addEventListener("resize", draw);
    document.addEventListener("keydown", e => {
      if (e.code === "Space" && !e.repeat && !e.target.closest("input, select, button, a, summary")) {
        e.preventDefault(); state.running = !state.running; updateReadouts();
      }
    });
  }

  function frame(now) {
    if (state.running && !document.hidden) {
      for (let i = 0; i < state.stepsPerFrame; i++) integrateOnce();
    }
    draw(); updateReadouts();
    state.frames++;
    if (now - state.fpsStart >= 750) {
      byId("fpsReadout").textContent = Math.round(state.frames * 1000 / (now - state.fpsStart)) + " FPS";
      state.frames = 0; state.fpsStart = now;
    }
    state.lastFrame = now;
    requestAnimationFrame(frame);
  }

  bind(); syncControlLabels(); reset(); audit(); requestAnimationFrame(frame);
})();
