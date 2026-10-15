# Mutual softened gravity: separate CPU/f64 research lane

`mutual-gravity.js` is an opt-in, clean-room JavaScript/IEEE-754 binary64
implementation of symmetric cell-cell interactions. It changes **none** of the
browser, native, GPU, WebAssembly, or prescribed-field defaults. Load it separately
as `GalaxyMutualGravity` in a browser or `require('./mutual-gravity.js')` in Node.
It is a numerical reference/research lane, not a GPU implementation or a claim of
an asymptotically optimal FMM.

```js
require('./barnes-hut.js');
const mutual = require('./mutual-gravity.js');
const bodies = GalaxyBarnesHut.makeDisc(512, 303);
const { values, stats, tree } = mutual.accelerations(bodies);
mutual.stepLeapfrog(bodies, 0.001); // explicitly selected shared-step KDK
```

The public methods are `accelerations`, `stepLeapfrog`, and `pairedExpansion`.
The last exposes fixed-centre coefficients for independent numerical verification;
it does not run a tree traversal. `DEFAULTS` is frozen. No global acceleration
correction, mean subtraction, momentum projection, or reference-force substitution
is applied.

## Model and paired-force derivation

As in the existing browser reference, positions are planar but gravity is the 3D
inverse-square law restricted to z=0, softened by a positive Plummer epsilon.
For a pair of masses the exact potential energy is

```
U_ij = -G m_i m_j / sqrt(|r_i-r_j|^2 + epsilon^2).
```

Set `R = Z_A - Z_B`, `x = r_i - Z_A`, `y = r_j - Z_B`, where Z is a
mass-weighted cell centre. Expand the positive scalar kernel
`h(R+x-y) = G / sqrt(|R+x-y|^2 + epsilon^2)` through total degree three
in `(x-y)`. Taking the gradient with respect to x yields acceleration; the
opposite derivative with respect to y uses exactly the same coefficients.
No unsoftened 3D harmonic formula is transplanted into the planar kernel.

With `s^2 = R.R + epsilon^2`, the derivatives are

```
D_i       = -G R_i / s^3
D_ij      = -G delta_ij / s^3 + 3G R_i R_j / s^5
D_ijk     = 3G (delta_ij R_k + delta_ik R_j + delta_jk R_i) / s^5
            - 15G R_i R_j R_k / s^7.
```

Let `Q_A = sum_i m_i x_i x_i^T / M_A`, with the analogous Q_B. In exact
arithmetic the dipoles vanish. Summing the *same* truncated pair expansion over
sources gives

```
a_A(x) = M_B [D + D2 x + (1/2) D3 : (x x^T + Q_B)]
a_B(y) = M_A [-D + D2 y - (1/2) D3 : (y y^T + Q_A)].
```

Therefore the cell-integrated forces are opposite:

```
F_A = M_A M_B [D + (1/2) D3 : (Q_A+Q_B)] = -F_B.
```

All sums and Cartesian indices above use just x and y. `pairedExpansion` returns
`{a, b}`, each a length-nine Float64Array ordered
`[ax, ay, Jxx, Jxy, Jyy, Kxxx, Kxxy, Kxyy, Kyyy]`, evaluated as
`a(u) = a0 + J u + K:uu / 2`. Input cells contain positive `mass`, finite
`comX`, `comY`, and mass-normalized `qxx`, `qxy`, `qyy`.
Finite-difference tests hold the centres fixed when differentiating the cubic
Taylor potential. This is a local expansion identity, **not** an assertion that
forces with changing expansion centres/topology derive from a globally smooth
approximate Hamiltonian.

## Tree and symmetric traversal

Each solve creates a fresh quadtree and computes mass, centre, normalized second
moments, and the maximum particle distance from that centre for every node. Centre
arithmetic uses offsets from a member particle to reduce translation cancellation.
Support radii are slightly enlarged for rounding. The acceptance test is

```
radius_A + radius_B < theta * |Z_A-Z_B|.
```

It is symmetric, based on unsoftened separation, and restricted to `0 <= theta <= 1`.
A root-root traversal decomposes self-pairs into unordered child pairs. For distinct
nodes, a rejected interaction splits the larger-radius splittable node. Thus every
particle pair is represented once, either by a shared accepted expansion or a
symmetric direct near-field term. Singleton pairs are direct; self-force is never
included. `theta=0` disables every aggregate acceptance.

Accepted pairs accumulate local quadratic acceleration polynomials in **both**
nodes. The downward pass shifts each parent's polynomial to a child's centre:
`a0' = a0 + Jd + K:dd/2`, `J' = J + K.d`, `K' = K`, then evaluates at leaves.
This avoids applying each distant cell interaction individually to every body.
The implementation still scans particles to build ancestor moments and uses JS
objects/arrays; it is not a claim of optimal storage or construction complexity.

`stats.direct` counts unordered particle pairs; `stats.approximated` counts shared
cell interactions (at least one nonsingleton); `stats.visited` counts node-pair
visits and `stats.propagated` counts parent-child shifts. These counts are not
comparable to the baseline's *directed* interactions without accounting for that
difference, and are not wall-time speedups.

## Scope, inputs, and limitations

Defaults: `theta=0.3`, `softening=0.03`, `G=1`, `bucket=4`, `maxDepth=40`.
Bodies are an array of at least two finite x/y positions and positive finite masses.
`accelerations` does not change them. `stepLeapfrog` accepts ordinary data objects
with own writable x/y fields and an own data mass field. Own vx/vy fields must also
be writable data fields; missing velocities require an extensible body and are
initialized to zero, without invoking inherited getters/setters. Every destination
is preflighted before computation, and only x/y/vx/vy are committed after the
complete numerical step succeeds. Frozen bodies, accessor-backed state fields,
and sealed bodies missing velocities are rejected without changing other bodies.
Unrelated fields are preserved. Proxy objects and objects that mutate themselves
through traps are unsupported; the transactional guarantee is for ordinary data
objects. Its returned forces/tree are at the final state.
A positive finite dt is required. Integration timestep selection remains the caller's
responsibility.

This lane deliberately requires strictly positive softening; squared softening must
be finite and nonzero. This excludes the old solver's zero-softening option and its
singular coincident-particle case. Nonfinite intermediate or output arithmetic is
rejected, not silently presented as a result. Extreme scales can overflow even if
individual inputs are finite; nondimensionalize such states. Kernel derivatives
use normalized displacement before inverse powers, so a two-body acceleration at
separation 1e108 is retained as about 1e-216 rather than lost through an underflowed
inverse cube. Accepted expansions additionally require representable positive
second/third derivative scales; `pairedExpansion` at that separation deliberately
throws because its required higher coefficients cannot be represented. This lane
fails closed outside that coefficient range rather than returning an all-zero
polynomial. Bucket is a positive
safe integer; maximum depth is an integer from 1 to 64. Coincident or terminal cells
are evaluated directly, and may cost quadratic work. No universal force-error bound
is promised by theta. A tree partition may change under rotation, so approximate
answers need not be bitwise rotation covariant; errors must still satisfy the same
independent direct-reference budgets.

Force balance does **not** establish angular-momentum conservation, exact-energy
conservation, long-term trajectory fidelity, symplecticity of approximate tree
forces, or speed. Shared-step KDK has finite timestep error, and tree acceptance can
change discontinuously. Exact softened energy and torque are measured separately.
The positive claim is limited to the executed fixture/parameter envelope.

## Validation and evidence protocol

Run from the repository root:

```
node tests/mutual-gravity.mjs
node tests/mutual-gravity-evidence.mjs
node scripts/bench-mutual-gravity.mjs --verify evidence/mutual-gravity-2026-10-15-r2
# New captures refuse to overwrite any existing evidence directory:
node scripts/bench-mutual-gravity.mjs --output runs/my-mutual-capture
```

The existing independent symmetric O(N^2) browser solver remains frozen and serves
as the full-force oracle. The existing reference gates are RMS relative force error
below 0.03 and maximum below 0.25 (`tests/barnes-hut.mjs`). Qualification additionally
requires normalized net internal force near f64 accumulation roundoff, with the
explicit tolerance `5e-13` recorded by the tests/harness. The large-translation
fixture (offsets 1e4 and -2e4) reaches about `1.40e-13`, while the ordinary
seeded force states stay below `4.31e-17`. Centre representation and mass-weighted
reductions are still floating-point operations; the claim is not bitwise zero or
translation-independent error at arbitrary coordinate scales. Near-zero reference accelerations
make pointwise relative errors ill-conditioned; also inspect globally scaled error
and absolute-force diagnostics.

A development pilot swept theta 0.3, 0.35, 0.4 and inspected 0.5 on deterministic
N=128/512/2048 discs/collisions (seeds 17/303/991). Theta 0.5 missed the RMS gate;
theta 0.4 missed the worst-point gate. Theta 0.3 was selected conservatively before
publishing the full qualification receipt. These are engineering conformance
fixtures, not a held-out scientific generalization study. Report this selection
rather than treating the receipt as independent hyperparameter validation.

The harness reports full-direct error distributions, normalized net force, torque,
shared-step trajectories, exact Plummer energy, and tighter-step controls. Timings
include the **entire** solve: fresh allocation, tree construction, moment gathering,
traversal, local coefficient accumulation, downward shifts, and force output. Baseline
and mutual opening parameters are calibrated by achieved force error, never treated
as equivalent just because theta values match. Finite grids may not produce an exact
match; the receipt must expose actual achieved error and unmatched cases. No speed
promotion is made by this PR, regardless of noisy timings. No hardware GPU execution
or claim is included. The new evidence directory preserves all historical evidence.

## Academic provenance

- Walter Dehnen, *A Very Fast and Momentum-Conserving Tree Code* (2000),
  ApJ Letters 536:L39–L42, DOI [10.1086/312724](https://doi.org/10.1086/312724),
  [author text](https://arxiv.org/html/astro-ph/0003209v2): symmetric acceptance,
  Cartesian mutual expansions, and downward local-coefficient propagation.
- Walter Dehnen, *A fast multipole method for stellar dynamics* (2014),
  [DOI 10.1186/s40668-014-0001-7](https://doi.org/10.1186/s40668-014-0001-7),
  [author record](https://arxiv.org/abs/1405.2255): motivates comparisons at achieved
  force accuracy and examination of error tails.

This reproduces established numerical ideas for GALAXY's softened planar contract;
it is not a new force law. The implementation and derivative algebra are written
here; no third-party implementation was copied. Published speed factors are not
predictions for GALAXY. Repository source remains Apache-2.0.
