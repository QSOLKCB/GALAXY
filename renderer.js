// SPDX-License-Identifier: MPL-2.0
(function (global) {
  "use strict";
  const Core = global.GalaxyCore;
  const VERTEX = `
    precision highp float;
    attribute vec4 a_first;
    attribute vec4 a_second;
    attribute float a_rate;
    uniform vec2 u_scale;
    uniform vec4 u_shape; // arms, pitch radians, scatter, bulge
    uniform vec4 u_view; // inclination radians, rotation radians, thickness, shear
    uniform float u_phase;
    uniform float u_size;
    uniform float u_density;
    uniform float u_exposure;
    uniform float u_palette;
    uniform float u_pointMax;
    varying mediump vec3 v_colour;
    varying mediump float v_light;
    void main() {
      float u = a_first.x, v = a_first.y, w = a_first.z;
      bool bulge = a_second.x < u_shape.w;
      bool halo = a_second.x >= 0.97;
      float r = bulge ? 0.015 + 0.27 * pow(u, 1.8)
        : halo ? 0.25 + 0.85 * u : 0.06 + 0.92 * pow(u, 1.4);
      float angle = 6.28318530718 * v;
      if (!bulge && !halo) {
        angle = floor(v * u_shape.x) * 6.28318530718 / u_shape.x
          + log(r / 0.1) / tan(u_shape.y) + (w - 0.5) * u_shape.z;
      }
      angle += u_phase * a_rate;
      float x = r * cos(angle), y = r * sin(angle);
      float z = (a_first.w - 0.5) * 2.0 * (bulge ? 0.19 * (1.0 - r / 0.3)
        : halo ? 0.4 : u_view.z * (0.4 + r));
      y = y * cos(u_view.x) + z * sin(u_view.x);
      vec2 point = vec2(x * cos(u_view.y) - y * sin(u_view.y),
        x * sin(u_view.y) + y * cos(u_view.y));
      gl_Position = vec4(point * u_scale, 0.0, 1.0);
      gl_PointSize = min(u_pointMax, (0.7 + 1.4 * pow(a_second.y, 6.0)) * u_size);
      vec3 warm = vec3(1.0, 0.77, 0.47), cool = vec3(0.54, 0.71, 1.0);
      v_colour = bulge ? mix(warm, vec3(1.0, 0.94, 0.81), a_second.w)
        : mix(cool, vec3(0.91, 0.95, 1.0), a_second.w);
      if (u_palette > 0.5 && u_palette < 1.5) v_colour = mix(warm, vec3(1.0, 0.94, 0.8), a_second.w);
      if (u_palette > 1.5) v_colour = vec3(0.84, 0.88, 0.94);
      v_light = (0.35 + a_second.z * 0.65) * u_density * u_exposure * (halo ? 0.6 : 1.0);
    }
  `;
  const FRAGMENT = `
    precision mediump float;
    varying mediump vec3 v_colour;
    varying mediump float v_light;
    void main() {
      vec2 p = gl_PointCoord * 2.0 - 1.0;
      float r2 = dot(p, p);
      if (r2 > 1.0) discard;
      float light = exp(-4.5 * r2) * v_light;
      gl_FragColor = vec4(v_colour * light, 1.0);
    }
  `;

  function colour(star, palette) {
    const mix = (a, b, t) => Math.round((a + (b - a) * t) * 255);
    const warm = [1, 0.77, 0.47], cool = [0.54, 0.71, 1];
    if (palette === "silver") return "rgb(214,224,240)";
    const a = star.bulge || palette === "ember" ? warm : cool;
    const b = star.bulge || palette === "ember" ? [1, 0.94, 0.81] : [0.91, 0.95, 1];
    return `rgb(${mix(a[0], b[0], star.tint)},${mix(a[1], b[1], star.tint)},${mix(a[2], b[2], star.tint)})`;
  }
  class CanvasRenderer {
    constructor(canvas) {
      this.canvas = canvas;
      this.context = canvas.getContext("2d", { alpha: false });
      if (!this.context) throw new Error("Canvas 2D is unavailable in this browser.");
      this.gpu = false;
      this.name = "Canvas 2D";
      this.count = 0;
      this.point = {};
      this.colours = [];
      this.colourKey = "";
    }
    setData(data) {
      this.data = data;
      this.count = data.length / Core.STRIDE;
      this.colourKey = "";
    }
    setRates(rates) {
      if (rates.length !== this.count) throw new Error("Orbit rate count does not match the star sample");
      this.rates = rates;
    }
    resize(width, height, dpr) { this.width = width; this.height = height; this.dpr = dpr; }
    draw(settings, phase) {
      const ctx = this.context, width = this.width, height = this.height;
      ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
      ctx.globalAlpha = 1;
      ctx.globalCompositeOperation = "source-over";
      ctx.fillStyle = "#030509";
      ctx.fillRect(0, 0, width, height);
      const key = settings.palette + ":" + settings.bulge;
      if (key !== this.colourKey) {
        this.colours = Array.from({ length: this.count }, (_, i) => colour(Core.starAt(this.data, i, settings, 0, this.point), settings.palette));
        this.colourKey = key;
      }
      const scale = Math.min(width, height) * 0.43 * settings.zoom;
      ctx.globalCompositeOperation = "lighter";
      // Cached colours and a reused point keep the bounded fallback allocation-free per star/frame.
      for (let i = 0; i < this.count; i++) {
        const p = Core.starAt(this.data, i, settings, phase, this.point, this.rates);
        const x = width / 2 + p.x * scale, y = height / 2 + p.y * scale;
        const radius = p.size * settings.starSize;
        ctx.fillStyle = this.colours[i];
        ctx.globalAlpha = Math.min(1, p.light * settings.exposure);
        ctx.beginPath(); ctx.arc(x, y, radius, 0, Core.TAU); ctx.fill();
        ctx.globalAlpha *= 0.12;
        ctx.beginPath(); ctx.arc(x, y, radius * 2.6, 0, Core.TAU); ctx.fill();
      }
      ctx.globalAlpha = 1;
      ctx.globalCompositeOperation = "source-over";
    }
    dispose() {}
  }

  class GLRenderer {
    constructor(canvas, gl) {
      this.canvas = canvas;
      this.gl = gl;
      this.gpu = true;
      this.name = "WebGL";
      this.count = 0;
      const shader = (kind, source) => {
        const object = gl.createShader(kind);
        gl.shaderSource(object, source); gl.compileShader(object);
        if (!gl.getShaderParameter(object, gl.COMPILE_STATUS)) {
          const message = gl.getShaderInfoLog(object); gl.deleteShader(object);
          throw new Error("Galaxy shader: " + message);
        }
        return object;
      };
      const vertex = shader(gl.VERTEX_SHADER, VERTEX);
      let fragment;
      try { fragment = shader(gl.FRAGMENT_SHADER, FRAGMENT); }
      catch (error) { gl.deleteShader(vertex); throw error; }
      const program = gl.createProgram();
      gl.attachShader(program, vertex); gl.attachShader(program, fragment); gl.linkProgram(program);
      gl.deleteShader(vertex); gl.deleteShader(fragment);
      if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
        const message = gl.getProgramInfoLog(program); gl.deleteProgram(program);
        throw new Error("Galaxy program: " + message);
      }
      this.program = program;
      this.buffer = gl.createBuffer();
      this.rateBuffer = gl.createBuffer();
      gl.useProgram(program);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.buffer);
      for (const [name, offset] of [["a_first", 0], ["a_second", 16]]) {
        const location = gl.getAttribLocation(program, name);
        gl.enableVertexAttribArray(location);
        gl.vertexAttribPointer(location, 4, gl.FLOAT, false, 32, offset);
      }
      gl.bindBuffer(gl.ARRAY_BUFFER, this.rateBuffer);
      const rateLocation = gl.getAttribLocation(program, "a_rate");
      gl.enableVertexAttribArray(rateLocation);
      gl.vertexAttribPointer(rateLocation, 1, gl.FLOAT, false, 4, 0);
      this.uniforms = {};
      for (const name of ["scale", "shape", "view", "phase", "size", "density", "exposure", "palette", "pointMax"]) this.uniforms[name] = gl.getUniformLocation(program, "u_" + name);
      gl.uniform1f(this.uniforms.pointMax, gl.getParameter(gl.ALIASED_POINT_SIZE_RANGE)[1]);
      gl.enable(gl.BLEND); gl.blendFunc(gl.ONE, gl.ONE);
      gl.disable(gl.DEPTH_TEST);
      gl.clearColor(3 / 255, 5 / 255, 9 / 255, 1);
    }
    setData(data) {
      const gl = this.gl;
      gl.bindBuffer(gl.ARRAY_BUFFER, this.buffer);
      gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
      this.count = data.length / Core.STRIDE;
    }
    setRates(rates) {
      if (rates.length !== this.count) throw new Error("Orbit rate count does not match the star sample");
      const gl = this.gl;
      gl.bindBuffer(gl.ARRAY_BUFFER, this.rateBuffer);
      gl.bufferData(gl.ARRAY_BUFFER, rates, gl.STATIC_DRAW);
    }
    resize(width, height, dpr) {
      this.width = width; this.height = height; this.dpr = dpr;
      this.gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    }
    draw(settings, phase) {
      const gl = this.gl, u = this.uniforms, rad = Math.PI / 180;
      const scale = Math.min(this.width, this.height) * 0.86 * settings.zoom;
      gl.useProgram(this.program); gl.clear(gl.COLOR_BUFFER_BIT);
      gl.uniform2f(u.scale, scale / this.width, -scale / this.height);
      gl.uniform4f(u.shape, settings.arms, settings.pitch * rad, settings.scatter, settings.bulge);
      gl.uniform4f(u.view, settings.inclination * rad, settings.rotation * rad, settings.thickness, settings.shear);
      gl.uniform1f(u.phase, phase);
      gl.uniform1f(u.size, 3.2 * this.dpr * settings.starSize);
      gl.uniform1f(u.density, Math.min(1.7, Math.sqrt(8192 / Math.max(this.count, 1))));
      gl.uniform1f(u.exposure, settings.exposure);
      gl.uniform1f(u.palette, settings.palette === "ember" ? 1 : settings.palette === "silver" ? 2 : 0);
      gl.drawArrays(gl.POINTS, 0, this.count);
    }
    dispose() {
      this.gl.deleteBuffer(this.buffer); this.gl.deleteBuffer(this.rateBuffer); this.gl.deleteProgram(this.program);
    }
  }
  function createRenderer(canvas, forceCanvas = false) {
    let fallbackReason = "";
    if (!forceCanvas) {
      try {
        const gl = canvas.getContext("webgl", { alpha: false, antialias: false, preserveDrawingBuffer: false, powerPreference: "high-performance" });
        if (gl) return { renderer: new GLRenderer(canvas, gl), canvas, fallbackReason };
        fallbackReason = "WebGL unavailable";
      } catch (error) { fallbackReason = error.message; }
    }
    // A canvas that acquired WebGL cannot then acquire a 2D context.
    const replacement = canvas.cloneNode(false);
    canvas.replaceWith(replacement);
    return { renderer: new CanvasRenderer(replacement), canvas: replacement, fallbackReason };
  }
  global.GalaxyRenderer = Object.freeze({ createRenderer, CanvasRenderer, GLRenderer, VERTEX, FRAGMENT });
})(globalThis);
