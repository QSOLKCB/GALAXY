// SPDX-License-Identifier: MPL-2.0
// Galaxy adaptation of VORTEX's offline controls, animation and local capture.
(async function () {
  "use strict";
  const Core = globalThis.GalaxyCore;
  const byId = id => document.getElementById(id);
  const state = {
    settings: Core.defaults(), clock: 0, lastTimestamp: null, dirty: true,
    wasm: null, data: null, renderer: null, canvas: byId("fieldCanvas"),
    limits: { logical: Core.MAX_LOGICAL_JS, rendered: Core.MAX_CANVAS_PARTICLES },
    recorder: null, recordingStream: null, recordingTimer: null,
    frames: 0, fpsStart: performance.now(), frameRequest: null
  };
  const profileNames = { grand: "Grand design spiral", pinwheel: "Pinwheel galaxy", flocculent: "Flocculent spiral", edge: "Edge-on galaxy", custom: "Custom galaxy" };
  const shapeKeys = new Set(["arms", "pitch", "scatter", "bulge", "thickness", "shear", "inclination", "rotation"]);
  const rangeKeys = ["arms", "pitch", "scatter", "bulge", "thickness", "speed", "shear", "phase", "inclination", "rotation", "zoom", "exposure", "starSize"];

  function status(message) { byId("status").textContent = message; }
  function capabilities() {
    state.limits = {
      logical: state.wasm ? Core.MAX_LOGICAL_RUST : Core.MAX_LOGICAL_JS,
      rendered: state.wasm && state.renderer.gpu ? Core.MAX_GPU_PARTICLES : Core.MAX_CANVAS_PARTICLES
    };
  }
  async function loadWasm() {
    if (!globalThis.WebAssembly || !globalThis.GALAXY_WASM_BASE64) throw new Error("Rust module unavailable");
    const bytes = Uint8Array.from(atob(globalThis.GALAXY_WASM_BASE64), c => c.charCodeAt(0));
    const { instance } = await WebAssembly.instantiate(bytes, {});
    const api = instance.exports;
    if (api.abi_version() !== 1 || api.max_rendered() !== Core.MAX_GPU_PARTICLES || !(api.memory instanceof WebAssembly.Memory)) {
      throw new Error("Unsupported Rust module");
    }
    return api;
  }
  function rebuild() {
    state.settings = Core.normalizeSettings(state.settings, state.limits);
    const s = state.settings;
    if (state.wasm) {
      try {
        const count = state.wasm.generate(s.logicalCount, s.renderedCount, s.seed);
        if (count !== s.renderedCount || state.wasm.buffer_len() !== count * Core.STRIDE) throw new Error("Invalid star buffer");
        // generate() can grow Wasm memory: never reuse an earlier memory view.
        state.data = new Float32Array(state.wasm.memory.buffer, state.wasm.buffer_ptr(), count * Core.STRIDE);
      } catch (error) {
        state.wasm = null;
        capabilities();
        status("Rust sampling stopped. Running the bounded JavaScript field.");
        rebuild();
        return;
      }
    } else state.data = Core.buildSample(s.logicalCount, s.renderedCount, s.seed);
    state.renderer.setData(state.data);
    state.dirty = true;
    syncControls();
  }

  function selectValue(id, value, maximum) {
    const select = byId(id);
    for (const option of [...select.options]) {
      if (option.dataset.imported) option.remove();
      else if (maximum !== undefined) option.disabled = Number(option.value) > maximum;
    }
    if (![...select.options].some(option => option.value === String(value))) {
      const option = document.createElement("option");
      option.value = String(value); option.textContent = Core.formatCount(value); option.dataset.imported = "true";
      select.appendChild(option);
    }
    select.value = String(value);
  }
  function syncControls() {
    const s = state.settings;
    for (const key of rangeKeys) {
      byId(key).value = String(s[key]);
      let value = s[key].toFixed(2);
      if (["arms"].includes(key)) value = String(s[key]);
      if (["pitch", "inclination", "rotation", "phase"].includes(key)) value = Math.round(s[key]) + "°";
      if (["bulge", "shear"].includes(key)) value = Math.round(s[key] * 100) + "%";
      if (["speed", "zoom", "exposure", "starSize"].includes(key)) value += "×";
      if (key === "thickness") value = s[key].toFixed(3);
      byId(key + "Value").textContent = value;
    }
    selectValue("logicalCount", s.logicalCount, state.limits.logical);
    selectValue("renderedCount", s.renderedCount, Math.min(state.limits.rendered, s.logicalCount));
    for (const key of ["profile", "palette", "evolution", "seed"]) byId(key).value = String(s[key]);
    byId("rotateCW").setAttribute("aria-pressed", String(s.direction === 1));
    byId("rotateCCW").setAttribute("aria-pressed", String(s.direction === -1));
    byId("playToggle").textContent = s.evolution === "phase" ? "Phase slice" : s.running ? "Pause" : "Play";
    byId("playToggle").disabled = s.evolution === "phase";
    byId("playToggle").setAttribute("aria-pressed", String(s.running && s.evolution !== "phase"));
    byId("fieldTitle").textContent = profileNames[s.profile];
    byId("motionBadge").textContent = s.shear === 0 ? "Stable spiral pattern" : "Differential rotation";
    byId("projectionBadge").textContent = "INCLINATION " + Math.round(s.inclination) + "°";
    byId("logicalReadout").textContent = Core.formatCount(s.logicalCount);
    byId("renderedReadout").textContent = Core.formatCount(state.renderer.count);
    byId("topologyReadout").textContent = Core.formatPowerOfTwo(s.logicalCount) + " indexed field";
    byId("memoryReadout").textContent = state.data.byteLength >= 1048576
      ? (state.data.byteLength / 1048576).toFixed(2) + " MiB" : (state.data.byteLength / 1024).toFixed(0) + " KiB";
    byId("engineStatus").textContent = (state.wasm ? "Rust / Wasm" : "JavaScript") + " + " + state.renderer.name;
    byId("capacityNote").textContent = "Available: " + Core.formatPowerOfTwo(state.limits.logical) +
      " logical / " + Core.formatCount(state.limits.rendered) + " rendered. " +
      (state.limits.rendered > 1024 ? "Large samples use the GPU." : "The VORTEX particle budget is retained.");
  }
  function resize() {
    const rect = byId("canvasStage").getBoundingClientRect();
    const width = Math.max(1, Math.floor(rect.width - 2)), height = Math.max(1, Math.floor(rect.height - 2));
    const dpr = Math.min(globalThis.devicePixelRatio || 1, 2);
    state.canvas.width = Math.round(width * dpr); state.canvas.height = Math.round(height * dpr);
    state.renderer.resize(width, height, dpr);
    state.dirty = true;
  }
  function togglePlay() {
    if (state.settings.evolution === "phase") return;
    state.settings.running = !state.settings.running;
    state.lastTimestamp = null; state.dirty = true; syncControls();
  }
  function bindCanvas() {
    const canvas = state.canvas;
    let drag = null;
    canvas.addEventListener("pointerdown", event => {
      if (!event.isPrimary || event.button !== 0) return;
      drag = { id: event.pointerId, x: event.clientX, y: event.clientY,
        rotation: state.settings.rotation, inclination: state.settings.inclination };
      canvas.setPointerCapture(event.pointerId);
    });
    canvas.addEventListener("pointermove", event => {
      if (!drag || drag.id !== event.pointerId) return;
      state.settings.rotation = Core.clamp(drag.rotation + (event.clientX - drag.x) * 0.3, -180, 180);
      state.settings.inclination = Core.clamp(drag.inclination + (event.clientY - drag.y) * 0.3, 0, 90);
      state.settings.profile = "custom"; state.dirty = true; syncControls();
    });
    const release = event => { if (drag?.id === event.pointerId) drag = null; };
    for (const name of ["pointerup", "pointercancel", "lostpointercapture"]) canvas.addEventListener(name, release);
    canvas.addEventListener("wheel", event => {
      event.preventDefault();
      state.settings.zoom = Core.clamp(state.settings.zoom * Math.exp(-event.deltaY * 0.001), 0.5, 1.8);
      state.dirty = true; syncControls();
    }, { passive: false });
    canvas.addEventListener("dblclick", () => {
      state.settings.inclination = 38; state.settings.rotation = -18; state.settings.zoom = 1;
      state.settings.profile = "custom"; state.dirty = true; syncControls();
    });
    canvas.addEventListener("webglcontextlost", event => {
      event.preventDefault();
      stopRecording();
      state.renderer.dispose();
      const result = globalThis.GalaxyRenderer.createRenderer(canvas, true);
      state.renderer = result.renderer; state.canvas = result.canvas;
      capabilities(); rebuild(); bindCanvas(); resize();
      status("Graphics context lost. Continuing with the 1,024-star Canvas fallback.");
    });
  }
  function downloadBlob(blob, filename) {
    const url = URL.createObjectURL(blob), anchor = document.createElement("a");
    anchor.href = url; anchor.download = filename;
    document.body.appendChild(anchor); anchor.click(); anchor.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  function snapshot() {
    // WebGL's back buffer is transient. Draw and request capture in one task.
    state.renderer.draw(state.settings, Core.effectivePhase(state.clock, state.settings));
    state.canvas.toBlob(blob => {
      if (blob) { downloadBlob(blob, "galaxy-" + state.settings.seed + ".png"); byId("captureStatus").textContent = "PNG saved locally."; }
      else byId("captureStatus").textContent = "The browser could not create the PNG.";
    }, "image/png");
  }
  function stopRecording() {
    if (state.recorder && state.recorder.state !== "inactive") state.recorder.stop();
  }
  function record() {
    if (state.recorder) { stopRecording(); return; }
    if (!state.canvas.captureStream || !globalThis.MediaRecorder) {
      byId("captureStatus").textContent = "WebM recording is unavailable in this browser. PNG capture is available."; return;
    }
    const mimeType = ["video/webm;codecs=vp9", "video/webm;codecs=vp8", "video/webm"].find(type => MediaRecorder.isTypeSupported(type));
    if (!mimeType) { byId("captureStatus").textContent = "This browser has no WebM encoder."; return; }
    let stream;
    try {
      stream = state.canvas.captureStream(30);
      const recorder = new MediaRecorder(stream, { mimeType, videoBitsPerSecond: 6000000 });
      const chunks = [], start = performance.now();
      const finish = () => {
        clearInterval(state.recordingTimer); state.recordingTimer = null;
        stream.getTracks().forEach(track => track.stop());
        state.recorder = null; state.recordingStream = null;
        byId("record").textContent = "Record WebM"; byId("recordingBadge").hidden = true;
      };
      let failed = false;
      recorder.addEventListener("dataavailable", event => { if (event.data.size) chunks.push(event.data); });
      recorder.addEventListener("error", () => { failed = true; finish(); byId("captureStatus").textContent = "Recording failed. Try a smaller rendered sample."; });
      recorder.addEventListener("stop", () => {
        finish();
        if (!failed && chunks.length) {
          downloadBlob(new Blob(chunks, { type: mimeType }), "galaxy-" + state.settings.seed + ".webm");
          byId("captureStatus").textContent = "WebM saved locally.";
        }
      });
      recorder.start(250); state.recorder = recorder; state.recordingStream = stream;
      byId("record").textContent = "Stop & save"; byId("recordingBadge").hidden = false;
      byId("recordingBadge").textContent = "REC 0s";
      byId("captureStatus").textContent = "Recording the galaxy…";
      state.recordingTimer = setInterval(() => {
        const elapsed = (performance.now() - start) / 1000;
        byId("recordingBadge").textContent = "REC " + Math.floor(elapsed) + "s";
        if (elapsed >= 30) stopRecording();
      }, 250);
      state.dirty = true;
    } catch (error) {
      stream?.getTracks().forEach(track => track.stop());
      byId("captureStatus").textContent = "Could not start recording: " + error.message;
    }
  }
  function bindControls() {
    byId("playToggle").addEventListener("click", togglePlay);
    byId("resetField").addEventListener("click", () => {
      state.clock = 0; state.settings.phase = 0; state.lastTimestamp = null; state.dirty = true; syncControls();
    });
    byId("profile").addEventListener("change", event => {
      const profile = event.target.value;
      if (Core.PRESETS[profile]) Object.assign(state.settings, Core.PRESETS[profile]);
      state.settings.profile = profile; state.clock = 0; state.settings.phase = 0;
      state.dirty = true; syncControls();
    });
    for (const key of rangeKeys) byId(key).addEventListener("input", event => {
      state.settings[key] = Number(event.target.value);
      if (shapeKeys.has(key)) state.settings.profile = "custom";
      state.dirty = true; syncControls();
    });
    for (const key of ["logicalCount", "renderedCount", "seed"]) byId(key).addEventListener("change", event => {
      state.settings[key] = event.target.value === "" ? 303 : Number(event.target.value);
      rebuild();
    });
    byId("pushLimit").addEventListener("click", () => {
      state.settings.logicalCount = state.limits.logical;
      state.settings.renderedCount = state.limits.rendered;
      rebuild(); status("Maximum field: " + Core.formatCount(state.settings.logicalCount) + " logical stars, " + Core.formatCount(state.renderer.count) + " rendered per frame.");
    });
    for (const [id, direction] of [["rotateCW", 1], ["rotateCCW", -1]]) byId(id).addEventListener("click", () => {
      state.settings.direction = direction; state.dirty = true; syncControls();
    });
    byId("evolution").addEventListener("change", event => {
      state.settings.evolution = event.target.value; state.lastTimestamp = null; state.dirty = true; syncControls();
    });
    byId("palette").addEventListener("change", event => { state.settings.palette = event.target.value; state.dirty = true; });
    byId("snapshot").addEventListener("click", snapshot);
    byId("record").addEventListener("click", record);
    byId("exportState").addEventListener("click", () => {
      downloadBlob(new Blob([JSON.stringify(Core.exportState(state.settings, state.clock), null, 2) + "\n"], { type: "application/json" }), "galaxy-" + state.settings.seed + "-settings.json");
      byId("captureStatus").textContent = "Galaxy settings and current phase saved locally.";
    });
    byId("importState").addEventListener("change", async event => {
      const file = event.target.files[0];
      if (!file) return;
      try {
        if (file.size > 65536) throw new Error("Settings files must be smaller than 64 KiB.");
        const document = JSON.parse(await file.text());
        const imported = Core.importState(document, state.limits);
        state.settings = imported.settings; state.clock = imported.clock; state.lastTimestamp = null;
        rebuild();
        const reduced = document.settings.logicalCount !== state.settings.logicalCount || document.settings.renderedCount !== state.settings.renderedCount;
        byId("captureStatus").textContent = reduced ? "Settings loaded; star counts reduced to this engine’s available capacity." : "Settings and phase restored.";
      } catch (error) { byId("captureStatus").textContent = "Could not load settings: " + error.message; }
      finally { event.target.value = ""; }
    });
    document.addEventListener("keydown", event => {
      if (event.code !== "Space" || event.repeat || event.ctrlKey || event.metaKey || event.altKey ||
        event.target.closest("input, select, textarea, button, a, summary, [contenteditable]")) return;
      event.preventDefault(); togglePlay();
    });
    document.addEventListener("visibilitychange", () => {
      state.lastTimestamp = null; state.fpsStart = performance.now(); state.frames = 0; state.dirty = true;
    });
  }
  function frame(timestamp) {
    if (!document.hidden) {
      const dt = state.lastTimestamp === null ? 0 : (timestamp - state.lastTimestamp) / 1000;
      state.lastTimestamp = timestamp;
      const clock = Core.advanceClock(state.clock, dt, state.settings);
      if (state.dirty || clock !== state.clock || state.recorder) {
        state.clock = clock;
        state.renderer.draw(state.settings, Core.effectivePhase(state.clock, state.settings));
        state.dirty = false; state.frames++;
      }
      if (timestamp - state.fpsStart >= 750) {
        byId("fpsReadout").textContent = state.settings.evolution === "phase" || !state.settings.running || state.settings.speed === 0
          ? "STILL FRAME" : Math.round(state.frames * 1000 / (timestamp - state.fpsStart)) + " FPS";
        state.frames = 0; state.fpsStart = timestamp;
      }
    } else state.lastTimestamp = null;
    state.frameRequest = requestAnimationFrame(frame);
  }

  try {
    const result = globalThis.GalaxyRenderer.createRenderer(state.canvas);
    state.renderer = result.renderer; state.canvas = result.canvas;
    let wasmReason = "";
    try { state.wasm = await loadWasm(); } catch (error) { wasmReason = error.message; }
    capabilities();
    if (globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches) state.settings.running = false;
    rebuild(); bindControls(); bindCanvas(); resize();
    if (globalThis.ResizeObserver) new ResizeObserver(resize).observe(byId("canvasStage"));
    window.addEventListener("resize", resize);
    window.addEventListener("pagehide", () => { stopRecording(); state.lastTimestamp = null; });
    status(state.wasm ? "Rust sampler ready. Every frame draws the displayed star count in a bounded working set."
      : "JavaScript fallback active: up to 2²⁴ logical stars and 1,024 rendered. " + (wasmReason.includes("unavailable") ? "The Rust module is unavailable." : "WebAssembly could not start in this browser."));
    if (result.fallbackReason && state.wasm) status("Rust sampler ready; using Canvas because WebGL is unavailable. Up to 2³² logical stars / 1,024 rendered.");
    state.frameRequest = requestAnimationFrame(frame);
  } catch (error) {
    status("The galaxy could not start: " + error.message);
    byId("engineStatus").textContent = "Startup failed";
    for (const control of document.querySelectorAll("button, input, select")) control.disabled = true;
  }
})();
