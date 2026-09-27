// SPDX-License-Identifier: Apache-2.0
// Execute the real solver and view with a small DOM/canvas adapter.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const read = name => fs.readFileSync(new URL('../' + name, import.meta.url), 'utf8');
class Element {
  constructor(tag = 'div') { this.tagName = tag; this.listeners = {}; this.value = ''; this.checked = false; this.textContent = ''; }
  addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); }
  emit(name, extra = {}) { for (const fn of this.listeners[name] || []) fn({target:this, preventDefault(){}, ...extra}); }
  setAttribute() {}
  getBoundingClientRect() { return {width:900, height:600}; }
  closest() { return null; }
  setPointerCapture() {}
}
function boot(reduced = false) {
  const elements = new Map(); let scheduled, bodies, steps = 0, draws = 0;
  for (const m of read('index.html').matchAll(/<(\w+)\b[^>]*\bid="([^"]+)"[^>]*>/g)) {
    const el = new Element(m[1]); el.value = m[0].match(/value="([^"]*)"/)?.[1] || '';
    el.checked = m[0].includes(' checked'); elements.set(m[2], el);
  }
  const paint = new Proxy({}, { get: (o,k) => k === 'createRadialGradient' ? () => ({addColorStop(){}}) : k === 'drawImage' ? () => {draws++;} : () => {}, set:(o,k,v)=>{o[k]=v;return true;} });
  elements.get('nbodyCanvas').getContext = () => paint;
  const doc = new Element(); doc.hidden = false; doc.getElementById = id => elements.get(id);
  doc.createElement = () => ({getContext:()=>paint});
  const motion = new Element(); motion.matches = reduced;
  const ctx = vm.createContext({document:doc, performance:{now:()=>0}, matchMedia:()=>motion,
    requestAnimationFrame(fn){scheduled=fn;}, devicePixelRatio:1, addEventListener(){}, console});
  vm.runInContext(read('barnes-hut.js'),ctx);
  const solver = ctx.GalaxyBarnesHut;
  ctx.GalaxyBarnesHut = {...solver, accelerations(b,o){bodies=b;return solver.accelerations(b,o);},
    stepLeapfrog(b,dt,o){steps++; bodies=b;return solver.stepLeapfrog(b,dt,o);} };
  vm.runInContext(read('nbody-clock.js'),ctx);
  vm.runInContext(read('nbody-viz.js'),ctx);
  return {elements, doc, motion, frame:now=>scheduled(now), steps:()=>steps, draws:()=>draws,
    bodies:()=>JSON.parse(JSON.stringify(bodies)), click:id=>elements.get(id).emit('click'),
    set(id,value,event='input'){elements.get(id).value=String(value);elements.get(id).emit(event);} };
}
function clockSteps(hz, speed) {
  const ctx = vm.createContext({});
  vm.runInContext(read('nbody-clock.js'), ctx);
  const clock = new ctx.GalaxyNBodyClock();
  let total = 0;
  for (let i = 0; i <= hz; i++) total += clock.advance(i * 1000 / hz, true, speed);
  return total;
}
assert.equal(clockSteps(50, 4), 240);
assert.equal(clockSteps(100, 4), 240);

// Same simulated wall time on 60 and 144 Hz displays gives the same trajectory.
const a=boot(), b=boot();
a.set('count',128,'change'); b.set('count',128,'change');
for(let i=0;i<=60;i++)a.frame(i*1000/60);
for(let i=0;i<=144;i++)b.frame(i*1000/144);
assert.equal(a.steps(),60); assert.equal(b.steps(),60); assert.deepEqual(a.bodies(),b.bodies());
a.click('playToggle'); const paused=a.bodies(); a.frame(1200);a.frame(2000);
assert.deepEqual(a.bodies(),paused);
a.set('tilt',70);a.set('rotation',90);a.set('zoom',2);
assert.deepEqual(a.bodies(),paused,'camera must not alter dynamics');
a.click('singleStep');assert.equal(a.steps(),61);assert.equal(a.elements.get('runBadge').textContent,'PAUSED');
a.click('audit');assert.match(a.elements.get('auditStatus').textContent,/Direct audit: 12 probes/);
a.frame(3000); assert.notEqual(a.elements.get('errorReadout').textContent,'—');
a.click('playToggle');a.frame(4000);a.frame(4020);
assert.equal(a.elements.get('errorReadout').textContent,'—','advancing invalidates audit');
a.doc.hidden=true; const hidden=a.steps();a.frame(100000); assert.equal(a.steps(),hidden);
a.doc.hidden=false;a.frame(200000);assert.equal(a.steps(),hidden,'hidden tab does not catch up');
a.frame(200017);assert.equal(a.steps(),hidden+1);
a.motion.emit('change',{matches:true});assert.equal(a.elements.get('runBadge').textContent,'PAUSED');
assert.equal(a.elements.get('trails').checked,false);
const reduced=boot(true);reduced.frame(0);reduced.frame(1000);assert.equal(reduced.steps(),0);
for(const preset of ['disc','collision','cold']){
 reduced.set('preset',preset,'change');const initial=reduced.bodies();
 reduced.click('singleStep');assert.ok(reduced.bodies().every(b=>[b.x,b.y,b.vx,b.vy].every(Number.isFinite)));
 reduced.click('reset');assert.deepEqual(reduced.bodies(),initial);
}
assert.equal(reduced.bodies().length,768);
// Assets must remain available for the default and both preserved instruments.
for(const page of ['index.html','rotation-lab.html','barnes-hut.html']) {
 for(const [,asset] of read(page).matchAll(/(?:src|href)="([^"#]+)"/g)) {
  if(!asset.startsWith('http'))assert.ok(fs.existsSync(new URL('../'+asset,import.meta.url)),asset);
 }
}
console.log('PASS: N-body refresh-rate invariance through 4x speed, pause/step, view isolation, audits, reduced motion, hidden tabs, presets and offline links.');
