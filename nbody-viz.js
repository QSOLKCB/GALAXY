// SPDX-License-Identifier: Apache-2.0
(function () {
  "use strict";
  const BH = globalThis.GalaxyBarnesHut;
  const byId = id => document.getElementById(id);
  const canvas = byId("nbodyCanvas");
  const ctx = canvas.getContext("2d", { alpha: false });
  if (!BH || !ctx) throw new Error("N-body simulator could not start");

  const motion = matchMedia("(prefers-reduced-motion: reduce)");
  const clock = new globalThis.GalaxyNBodyClock();
  const history = [];
  const sprites = [makeSprite("224,187,126"), makeSprite("206,211,220")];
  let lastReadout = 0, previousDrawStep = -1;
  const state = {
    bodies: [], latest: null, running: !motion.matches, step: 0, lastFrame: 0, fpsStart: performance.now(), frames: 0,
    preset: "collision", count: 768, seed: 303, theta: 0.5, softening: 0.03, bucket: 4,
    dt: 0.004, stepsPerFrame: 1, showTree: false, treeDepth: 5, probeRms: null, simulatedTime: 0, tilt: 38, rotation: -18, zoom: 1, light: 1, trails: !motion.matches, view: { cx: 0, cy: 0, span: 3 }
  };

  function makeSprite(rgb) {
    const sprite = document.createElement("canvas");
    sprite.width = sprite.height = 64;
    const paint = sprite.getContext("2d");
    const glow = paint.createRadialGradient(32, 32, 0, 32, 32, 32);
    glow.addColorStop(0, "rgba(255,249,228,1)");
    glow.addColorStop(0.07, `rgba(${rgb},0.85)`);
    glow.addColorStop(0.22, `rgba(${rgb},0.25)`);
    glow.addColorStop(0.6, `rgba(${rgb},0.045)`);
    glow.addColorStop(1, `rgba(${rgb},0)`);
    paint.fillStyle = glow; paint.fillRect(0, 0, 64, 64);
    return sprite;
  }

  function remember() {
    const points = new Float64Array(state.bodies.length * 2);
    state.bodies.forEach((b, i) => { points[i * 2] = b.x; points[i * 2 + 1] = b.y; });
    history.push(points);
    if (history.length > 10) history.shift();
  }

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
    state.simulatedTime = 0;
    state.view = extent(); state.zoom = 1;
    byId("zoom").value = 1;
    history.length = 0; remember(); clock.reset();
    syncControlLabels();
    invalidateAudit();
    byId("presetTitle").textContent = presetName();
    byId("viewStatus").textContent = `${state.bodies.length.toLocaleString()} bodies, each contributing mass. Glow and trails add depth without adding gravitational work.`;
    updateReadouts(); draw();
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
    const w = canvas.width, h = canvas.height, view = state.view;
    const dpr = Math.min(globalThis.devicePixelRatio || 1, 2);
    const scale = Math.min(w, h) / view.span * state.zoom;
    const angle = state.rotation * Math.PI / 180;
    const c = Math.cos(angle), sn = Math.sin(angle), tilt = Math.cos(state.tilt * Math.PI / 180);
    const project = (x, y) => {
      x -= view.cx; y -= view.cy;
      return [w * 0.5 + (x * c - y * sn) * scale, h * 0.5 - (x * sn + y * c) * tilt * scale];
    };
    ctx.fillStyle = "#020304"; ctx.fillRect(0, 0, w, h);
    // Faint reference rings supply depth without extra simulated particles.
    ctx.strokeStyle = "rgba(151,143,126,0.08)"; ctx.lineWidth = dpr * 0.65;
    for (const radius of [0.5, 1, 1.5]) {
      ctx.beginPath();
      for (let k = 0; k <= 96; k++) {
        const a = k / 96 * Math.PI * 2;
        const [x, y] = project(view.cx + radius * Math.cos(a), view.cy + radius * Math.sin(a));
        if (k === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
      }
      ctx.stroke();
    }
    if (state.showTree && state.latest) {
      ctx.lineWidth = dpr * 0.6;
      for (const node of BH.flattenTree(state.latest.tree, state.treeDepth)) {
        if (node.depth === 0) continue;
        ctx.strokeStyle = `rgba(214,173,103,${Math.max(0.05, 0.3 - node.depth * 0.035)})`;
        ctx.beginPath();
        for (const [i, [dx, dy]] of [[-1,-1], [1,-1], [1,1], [-1,1]].entries()) {
          const [x, y] = project(node.cx + dx * node.half, node.cy + dy * node.half);
          if (!i) ctx.moveTo(x,y); else ctx.lineTo(x,y);
        }
        ctx.closePath(); ctx.stroke();
      }
    }
    ctx.globalCompositeOperation = "lighter";
    if (state.trails && history.length > 1) {
      ctx.lineWidth = 0.7 * dpr;
      for (let k = 1; k < history.length; k++) {
        ctx.strokeStyle = `rgba(198,180,150,${0.18 * k / history.length * state.light})`;
        ctx.beginPath();
        for (let i = 0; i < state.bodies.length; i++) {
          const [x0,y0] = project(history[k-1][i*2], history[k-1][i*2+1]);
          const [x1,y1] = project(history[k][i*2], history[k][i*2+1]);
          ctx.moveTo(x0,y0); ctx.lineTo(x1,y1);
        }
        ctx.stroke();
      }
    }
    for (let i = 0; i < state.bodies.length; i++) {
      const b = state.bodies[i];
      const [x, y] = project(b.x, b.y);
      const size = (10 + (i % 7) * 1.5) * dpr * Math.sqrt(state.light);
      ctx.globalAlpha = 0.65 + (i % 5) * 0.07;
      const group = state.preset === "collision" ? (i < state.bodies.length / 2 ? 0 : 1) : i % 2;
      ctx.drawImage(sprites[group], x - size/2, y - size/2, size, size);
    }
    ctx.globalAlpha = 1; ctx.globalCompositeOperation = "source-over";
    previousDrawStep = state.step;
  }

  function updateReadouts() {
    const n = state.bodies.length;
    byId("bodyReadout").textContent = formatInt(n);
    byId("thetaBadge").textContent = "θ " + state.theta.toFixed(2);
    byId("timeReadout").textContent = "t " + state.simulatedTime.toFixed(3) + " · STEP " + formatInt(state.step);
    if (state.latest) {
      const stats = state.latest.stats;
      const terms = stats.direct + stats.approximated;
      const exactTerms = n * (n - 1);
      const reduction = exactTerms ? Math.max(0, 1 - terms / exactTerms) : 0;
      byId("nodeReadout").textContent = formatInt(state.latest.tree.nodeCount);
      byId("depthReadout").textContent = (state.latest.tree.maxDepth + 1) + " levels · " + formatInt(state.latest.tree.leafCount) + " leaves";
      byId("termReadout").textContent = formatInt(terms);
      byId("reductionReadout").textContent = (reduction * 100).toFixed(1) + "% fewer force terms than direct";
    }
    byId("errorReadout").textContent = state.probeRms === null ? "—" : (state.probeRms * 100).toFixed(2) + "%";
    byId("runBadge").textContent = state.running ? "RUNNING" : "PAUSED";
    byId("playToggle").textContent = state.running ? "Pause" : "Resume";
    byId("playToggle").setAttribute("aria-pressed", String(state.running));
  }

  function invalidateAudit(message = "Run the direct-force probe for the current state.") {
    state.probeRms = null;
    byId("errorReadout").textContent = "—";
    byId("auditStatus").textContent = message;
  }

  function integrateOnce() {
    state.latest = BH.stepLeapfrog(state.bodies, state.dt, options());
    state.step++;
    state.simulatedTime += state.dt;
    if (state.trails) remember();
    invalidateAudit("State advanced; run the direct-force probe again for this step.");
  }

  function audit() {
    if (!state.latest) return;
    state.running = false; clock.reset();
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
    byId("stepsValue").textContent = state.stepsPerFrame + "×";
    byId("tiltValue").textContent = state.tilt + "°";
    byId("rotationValue").textContent = state.rotation + "°";
    byId("zoomValue").textContent = state.zoom.toFixed(1) + "×";
    byId("lightValue").textContent = state.light.toFixed(1) + "×";
    byId("trails").checked = state.trails;
  }

  function bind() {
    byId("playToggle").addEventListener("click", () => { state.running = !state.running; clock.reset(); updateReadouts(); });
    byId("singleStep").addEventListener("click", () => { state.running = false; clock.reset(); integrateOnce(); draw(); updateReadouts(); });
    byId("reset").addEventListener("click", reset);
    byId("audit").addEventListener("click", audit);
    byId("preset").addEventListener("change", e => { state.preset = e.target.value; reset(); });
    byId("count").addEventListener("input", e => { state.count = Number(e.target.value); syncControlLabels(); });
    byId("count").addEventListener("change", reset);
    byId("seed").addEventListener("change", e => { state.seed = Math.min(4294967295, Math.max(1, Math.trunc(Number(e.target.value) || 303))); e.target.value = state.seed; reset(); });
    byId("theta").addEventListener("input", e => { state.theta = Number(e.target.value); syncControlLabels(); state.latest = BH.accelerations(state.bodies, options()); invalidateAudit("Opening angle changed; run the direct-force probe again."); updateReadouts(); draw(); });
    byId("softening").addEventListener("input", e => { state.softening = Number(e.target.value); syncControlLabels(); state.latest = BH.accelerations(state.bodies, options()); invalidateAudit("Softening changed; run the direct-force probe again."); updateReadouts(); draw(); });
    byId("bucket").addEventListener("input", e => { state.bucket = Number(e.target.value); syncControlLabels(); state.latest = BH.accelerations(state.bodies, options()); invalidateAudit("Leaf bucket changed; run the direct-force probe again."); updateReadouts(); draw(); });
    byId("showTree").addEventListener("change", e => { state.showTree = e.target.checked; draw(); });
    byId("treeDepth").addEventListener("input", e => { state.treeDepth = Number(e.target.value); syncControlLabels(); draw(); });
    byId("dt").addEventListener("input", e => { state.dt = Number(e.target.value); syncControlLabels(); });
    byId("steps").addEventListener("input", e => { state.stepsPerFrame = Number(e.target.value); syncControlLabels(); });
    byId("fitView").addEventListener("click", () => { state.view = extent(); state.zoom = 1; byId("zoom").value = 1; syncControlLabels(); draw(); });
    for (const name of ["tilt", "rotation", "zoom", "light"]) {
      byId(name).addEventListener("input", e => { state[name] = Number(e.target.value); syncControlLabels(); draw(); });
    }
    byId("trails").addEventListener("change", e => { state.trails = e.target.checked; history.length = 0; remember(); draw(); });
    let pointer = null;
    canvas.addEventListener("pointerdown", e => { pointer = { id: e.pointerId, x: e.clientX, y: e.clientY }; canvas.setPointerCapture(e.pointerId); });
    canvas.addEventListener("pointermove", e => {
      if (!pointer || pointer.id !== e.pointerId) return;
      state.rotation = Math.round(((state.rotation + (e.clientX - pointer.x) * 0.4 + 540) % 360) - 180);
      state.tilt = Math.round(Math.max(0, Math.min(80, state.tilt + (e.clientY - pointer.y) * 0.3)));
      pointer.x = e.clientX; pointer.y = e.clientY;
      byId("rotation").value = state.rotation; byId("tilt").value = state.tilt;
      syncControlLabels(); draw();
    });
    for (const event of ["pointerup", "pointercancel", "lostpointercapture"]) canvas.addEventListener(event, () => { pointer = null; });
    canvas.addEventListener("wheel", e => {
      e.preventDefault(); state.zoom = Math.max(0.3, Math.min(3, state.zoom * Math.exp(-e.deltaY * 0.001)));
      byId("zoom").value = state.zoom; syncControlLabels(); draw();
    }, { passive: false });
    document.addEventListener("visibilitychange", () => {
      clock.reset();
      state.frames = 0;
      state.fpsStart = performance.now();
    });
    motion.addEventListener("change", e => {
      if (e.matches) { state.running = false; state.trails = false; clock.reset(); history.length = 0; syncControlLabels(); updateReadouts(); draw(); }
    });
    globalThis.addEventListener("resize", draw);
    document.addEventListener("keydown", e => {
      if (e.code === "Space" && !e.repeat && !e.target.closest("input, select, button, a, summary")) {
        e.preventDefault(); state.running = !state.running; clock.reset(); updateReadouts();
      }
    });
  }

  function frame(now) {
    const steps = clock.advance(now, state.running && !document.hidden, state.stepsPerFrame);
    try {
      for (let i = 0; i < steps; i++) integrateOnce();
    } catch (error) {
      state.running = false; clock.reset();
      byId("viewStatus").textContent = "Simulation stopped: " + error.message + ". Reduce the time step, then reset.";
    }
    if (!document.hidden) {
      if (previousDrawStep !== state.step) draw();
      if (now - lastReadout > 200 || !state.running) { updateReadouts(); lastReadout = now; }
      state.frames++;
      if (now - state.fpsStart >= 750) {
        byId("fpsReadout").textContent = Math.round(state.frames * 1000 / (now - state.fpsStart)) + " FPS";
        state.frames = 0; state.fpsStart = now;
      }
    }
    requestAnimationFrame(frame);
  }

  bind(); syncControlLabels(); reset(); requestAnimationFrame(frame);
})();
