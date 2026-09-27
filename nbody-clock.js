// SPDX-License-Identifier: Apache-2.0
// Bounded fixed-rate scheduling; never enlarge dt to catch up with rendering.
(function (global) {
  "use strict";
  const MAX_SPEED = 4;
  const MAX_ELAPSED_SECONDS = 0.1;
  const MAX_STEPS_PER_FRAME = Math.ceil(60 * MAX_SPEED * MAX_ELAPSED_SECONDS);
  class FixedClock {
    constructor() { this.reset(); }
    reset() { this.last = null; this.remainder = 0; }
    advance(now, active, speed = 1) {
      if (!active) { this.reset(); return 0; }
      if (this.last === null) { this.last = now; return 0; }
      const elapsed = Math.max(0, Math.min(MAX_ELAPSED_SECONDS, (now - this.last) / 1000));
      this.last = now;
      this.remainder += elapsed * 60 * speed;
      const due = Math.floor(this.remainder + 1e-9);
      const steps = Math.min(MAX_STEPS_PER_FRAME, due);
      this.remainder -= due; // Drop overload debt; no unbounded catch-up burst.
      return steps;
    }
  }
  global.GalaxyNBodyClock = FixedClock;
})(globalThis);
