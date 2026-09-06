// SPDX-License-Identifier: Apache-2.0
// Numeric plot of the selected UFF law, baryonic input, and the bundled demo rows.
(function (global) {
  "use strict";
  class RotationCurve {
    constructor(canvas) { this.canvas = canvas; this.context = canvas.getContext("2d"); }
    draw(settings) {
      if (settings.dynamics === "legacy" || !this.context) return;
      const Physics = global.UffPhysics;
      const width = Math.max(280, this.canvas.getBoundingClientRect().width);
      const height = 220, dpr = Math.min(global.devicePixelRatio || 1, 2);
      this.canvas.width = Math.round(width * dpr); this.canvas.height = height * dpr;
      const ctx = this.context;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.clearRect(0, 0, width, height);
      const left = 48, right = width - 18, top = 26, bottom = height - 35;
      const maxRadius = 13.2, rows = Physics.Data.rows;
      const curve = Array.from({ length: 257 }, (_, i) => {
        const radius = 0.18 + (maxRadius - 0.18) * i / 256;
        return { radius, ...Physics.velocityComponents(radius, settings) };
      });
      const maxVelocity = Math.max(...curve.map(p => p.total), ...rows.map(row => row[1] + row[2]));
      const yMax = Math.ceil(maxVelocity / 40) * 40 || 40;
      const x = r => left + r / maxRadius * (right - left);
      const y = v => bottom - v / yMax * (bottom - top);
      ctx.fillStyle = "#121821";
      ctx.fillRect(x(0), top, x(0.5) - x(0), bottom - top);
      ctx.fillRect(x(12), top, x(13.2) - x(12), bottom - top);
      ctx.font = "10px monospace"; ctx.lineWidth = 1;
      for (let tick = 0; tick <= 4; tick++) {
        const velocity = yMax * tick / 4;
        ctx.strokeStyle = "#252b34"; ctx.beginPath(); ctx.moveTo(left, y(velocity)); ctx.lineTo(right, y(velocity)); ctx.stroke();
        ctx.fillStyle = "#8c929d"; ctx.textAlign = "right"; ctx.fillText(velocity.toFixed(0), left - 9, y(velocity) + 3);
      }
      for (const radius of [0, 3, 6, 9, 12]) {
        ctx.fillStyle = "#8c929d"; ctx.textAlign = "center"; ctx.fillText(String(radius), x(radius), bottom + 16);
      }
      ctx.textAlign = "left"; ctx.fillText("km/s", left, 13);
      ctx.textAlign = "right"; ctx.fillText("radius / kpc", right, height - 5);
      const line = (key, colour, dashed) => {
        ctx.strokeStyle = colour; ctx.lineWidth = 1.7; ctx.setLineDash(dashed ? [5, 4] : []); ctx.beginPath();
        curve.forEach((point, index) => { if (index === 0) ctx.moveTo(x(point.radius), y(point[key])); else ctx.lineTo(x(point.radius), y(point[key])); });
        ctx.stroke(); ctx.setLineDash([]);
      };
      line("baryons", "#8295a9", true); line("total", "#d7b47c", false);
      ctx.strokeStyle = "#bfc3cb"; ctx.fillStyle = "#bfc3cb"; ctx.lineWidth = 1;
      for (const [radius, velocity, error] of rows) {
        ctx.beginPath(); ctx.moveTo(x(radius), y(velocity - error)); ctx.lineTo(x(radius), y(velocity + error));
        ctx.moveTo(x(radius) - 3, y(velocity - error)); ctx.lineTo(x(radius) + 3, y(velocity - error));
        ctx.moveTo(x(radius) - 3, y(velocity + error)); ctx.lineTo(x(radius) + 3, y(velocity + error)); ctx.stroke();
        ctx.beginPath(); ctx.arc(x(radius), y(velocity), 2.2, 0, Math.PI * 2); ctx.fill();
      }
      this.canvas.setAttribute("aria-label", Physics.LABELS[settings.dynamics] + " circular velocity compared with baryons and six UFF demo rows; radius in kpc and speed in km/s.");
    }
  }
  global.GalaxyRotationCurve = RotationCurve;
})(globalThis);
