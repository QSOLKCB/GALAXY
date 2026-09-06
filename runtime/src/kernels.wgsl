// SPDX-License-Identifier: Apache-2.0
// UFF equations: see data/uff/NOTICE. DEMO_* and unit constants are generated
// from the pinned source CSV at build time. Stable series suit float32 kernels.
struct Global {
    info: vec4<u32>,       // particle/case count, seed, completed steps, sample count
    motion: vec4<f32>,     // dt Myr, time Myr, direction, softening kpc
    shape: vec4<f32>,      // arms, pitch radians, scatter, visual bulge fraction
    extra: vec4<f32>,      // thickness kpc, radial kick km/s, integrator (0/1), unused
}
struct Case { info: vec4<u32>, p0: vec4<f32>, p1: vec4<f32>, p2: vec4<f32> }
struct Particle { orbit: vec4<f32>, state: vec4<f32> }
@group(0) @binding(0) var<uniform> settings: Global;
@group(0) @binding(1) var<storage, read> cases: array<Case>;
@group(0) @binding(2) var<storage, read_write> particles: array<Particle>;
@group(0) @binding(3) var<storage, read_write> outputs: array<Particle>;
const PI: f32 = 3.141592653589793;
const TAU: f32 = 6.283185307179586;

fn components(r: f32) -> vec3<f32> {
    // Function-local arrays allow dynamic indexing on every supported backend.
    var radii = DEMO_R; var values = DEMO_V;
    if r <= radii[0] { return values[0]; }
    for (var i = 1u; i < DEMO_N; i++) {
        if r <= radii[i] { return mix(values[i-1u], values[i], (r-radii[i-1u])/(radii[i]-radii[i-1u])); }
    }
    return values[DEMO_N-1u];
}
fn nfw_shape(x: f32) -> f32 {
    if x < 0.1 { return x*x*(0.5+x*(-2.0/3.0+x*(0.75+x*(-0.8+x*(5.0/6.0-x*6.0/7.0))))); }
    return log(1.0+x)-x/(1.0+x);
}
fn velocity(r: f32, p: Case) -> f32 {
    let v = components(r);
    var total = max(0.0, v.x*abs(v.x)+p.p0.x*v.y*v.y+p.p0.y*v.z*v.z) + G*p.p0.z*1e6/r;
    switch p.info.x {
        case 2u: {
            let mass = pow(10.0, p.p1.z); let c = p.p1.w;
            let rho = 3.0*0.07*0.07/(8.0*PI*G);
            let r200 = pow(3.0*mass/(4.0*PI*200.0*rho), 1.0/3.0);
            total += G*mass*nfw_shape(c*r/r200)/nfw_shape(c)/r;
        }
        case 3u: {
            let x = r/p.p2.y; var shape: f32;
            if x < 0.001 { shape = 4.0/3.0*x*x*x; }
            else if x < 0.5 {
                let q = x*x*x*x;
                shape = x*x*x*(4.0/3.0-x+q*(4.0/7.0-0.5*x+q*(4.0/11.0-x/3.0+q*(4.0/15.0-0.25*x+q*(4.0/19.0-0.2*x)))));
            }
            else { shape = log((1.0+x)*(1.0+x)*(1.0+x*x))-2.0*atan(x); }
            total += G*PI*pow(10.0,p.p2.x)*pow(p.p2.y,3.0)*shape/r;
        }
        case 4u: {
            if total > 0.0 {
                let root = sqrt(total*1e6/(r*3.085677581491367e19)/(p.p2.z*1e-10));
                var denominator: f32;
                if root < 0.1 { denominator = root*(1.0+root*(-0.5+root*(1.0/6.0+root*(-1.0/24.0+root/120.0)))); }
                else { denominator = 1.0-exp(-root); }
                total /= denominator;
            }
        }
        case 5u: {
            let x = r/p.p1.x; let q = x*x; var shape: f32;
            if x < 0.15 { shape = q*(1.0/3.0+q*(-0.2+q*(1.0/7.0+q*(-1.0/9.0+q/11.0)))); }
            else { shape = max(0.0,1.0-atan(x)/x); }
            total += p.p0.w*p.p0.w*shape*exp(2.0*p.p1.y*x/(1.0+x));
        }
        default: {}
    }
    return sqrt(total);
}
fn hash(value: u32) -> u32 {
    var x = value; x ^= x >> 16u; x *= 0x7feb352du; x ^= x >> 15u; x *= 0x846ca68bu; return x ^ (x >> 16u);
}
fn random(index: u32, lane: u32) -> f32 {
    return f32(hash(index ^ settings.info.y ^ ((lane+1u)*0x9e3779b9u)) >> 8u)/16777216.0;
}
@compute @workgroup_size(256)
fn initialize(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x; if i >= settings.info.x { return; }
    let u = random(i,0u); let v = random(i,1u); let w = random(i,2u); let kind = random(i,4u);
    let bulge = kind < settings.shape.w; let halo = kind >= 0.97;
    var r = 0.06+0.92*pow(u,1.4);
    if bulge { r = 0.015+0.27*pow(u,1.8); } else if halo { r = 0.25+0.85*u; }
    var theta = TAU*v;
    if !bulge && !halo { theta = floor(v*settings.shape.x)*TAU/settings.shape.x+log(r/0.1)/tan(settings.shape.y)+(w-0.5)*settings.shape.z; }
    var height = settings.extra.x*(0.4+r);
    if bulge { height = 2.28*(1.0-r/0.3); } else if halo { height = 4.8; }
    let z = (random(i,3u)-0.5)*2.0*height;
    r *= 12.0;
    var speed = velocity(r,cases[0])*KMS_TO_KPC_MYR;
    if settings.extra.z > 0.5 {
        let soft_r = sqrt(r*r+settings.motion.w*settings.motion.w);
        speed = velocity(soft_r,cases[0])*KMS_TO_KPC_MYR*r/soft_r;
    }
    let omega = settings.motion.z*speed/r; let kick = settings.extra.y*KMS_TO_KPC_MYR;
    particles[i] = Particle(vec4<f32>(r,theta,z,omega),vec4<f32>(r*cos(theta),r*sin(theta),-omega*r*sin(theta)+kick*cos(theta),omega*r*cos(theta)+kick*sin(theta)));
}
@compute @workgroup_size(256)
fn circular(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x; if i >= settings.info.x { return; }
    let orbit = particles[i].orbit; let theta = orbit.y+settings.motion.y*orbit.w;
    particles[i].state = vec4<f32>(orbit.x*cos(theta),orbit.x*sin(theta),-orbit.w*orbit.x*sin(theta),orbit.w*orbit.x*cos(theta));
}
fn acceleration(point: vec2<f32>) -> vec2<f32> {
    let r = sqrt(dot(point,point)+settings.motion.w*settings.motion.w);
    let speed = velocity(r,cases[0])*KMS_TO_KPC_MYR;
    return -(speed*speed/(r*r))*point;
}
@compute @workgroup_size(256)
fn leapfrog(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x; if i >= settings.info.x { return; }
    let state = particles[i].state; let dt = settings.motion.x;
    let half_velocity = state.zw+0.5*dt*acceleration(state.xy);
    let point = state.xy+dt*half_velocity;
    particles[i].state = vec4<f32>(point,half_velocity+0.5*dt*acceleration(point));
}
@compute @workgroup_size(256)
fn gather(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x; let count = settings.info.w; let total = settings.info.x;
    if i >= count { return; }
    // Exact u32 mapping without the overflow in i*total at millions of stars.
    let index = i*(total/count)+(i*(total%count))/count;
    outputs[i] = particles[index];
}
@compute @workgroup_size(256)
fn curves(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x; if i >= settings.info.x { return; }
    let p = cases[i]; let r = p.p2.w; let v = velocity(r,p);
    outputs[i] = Particle(vec4<f32>(v,v*v*1e6/(r*3.085677581491367e19),TAU*r/(v*KMS_TO_KPC_MYR),0.0),vec4<f32>(0.0));
}
@compute @workgroup_size(256)
fn compact(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x; if i >= settings.info.x { return; }
    let mass = cases[i].p0.x; let spin = cases[i].p0.y;
    let z1 = 1.0+pow(1.0-spin*spin,1.0/3.0)*(pow(1.0+spin,1.0/3.0)+pow(1.0-spin,1.0/3.0));
    let z2 = sqrt(3.0*spin*spin+z1*z1);
    let horizon = 1.0+sqrt(1.0-spin*spin);
    let photon = 2.0*(1.0+cos((2.0/3.0)*acos(-spin)));
    let isco = 3.0+z2-sign(spin)*sqrt(max(0.0,(3.0-z1)*(3.0+z1+2.0*z2)));
    outputs[i] = Particle(vec4<f32>(horizon,photon,isco,G*mass/(299792.458*299792.458)),vec4<f32>(0.0));
}
