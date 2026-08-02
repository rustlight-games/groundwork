//! The bend field.
//!
//! One world-aligned grid carrying the state of every patch of grass. Blades
//! are not simulated individually — a cell holds the average posture of the
//! canopy above it, and the renderer reconstructs however many blades it needs
//! from that. Cost therefore scales with the *area* being simulated, not with
//! how much grass is drawn on it, which is what makes a densely grassed
//! battlefield affordable at all.
//!
//! ## What a cell remembers
//!
//! Six channels, and the reason there are six rather than one is that a single
//! displacement value cannot tell these situations apart:
//!
//! | Channel | Question it answers |
//! |---|---|
//! | `theta` | Which way is the grass leaning right now, and how far? |
//! | `omega` | How fast is it moving? — inertia, overshoot, spring-back |
//! | `fast_memory` | Where was it just pushed? — a wake that closes in a second |
//! | `slow_memory` | Where has it been pushed repeatedly? — a trail |
//! | `compaction` | How crushed is it, regardless of direction? |
//! | `axis` | Along which *axis* was it crushed? |
//!
//! That last one is the unusual one, and it earns its place. Suppose one unit
//! walks east along a path and another later walks west along the same path.
//! Averaging directions gives `(1,0) + (-1,0) = 0`, which claims the grass has
//! no preferred direction — but anyone looking at it can see a flattened track.
//! The axis channel stores orientation without sign, by accumulating the outer
//! product of the contact direction with itself in its compact double-angle
//! form `(cos 2phi, sin 2phi)`. Opposite directions then reinforce instead of
//! cancelling, while perpendicular ones cancel, which is exactly right: a path
//! walked both ways is strongly aligned, and a patch trampled from every angle
//! is crushed without alignment.
//!
//! ## Why the solve is implicit
//!
//! Contact springs are stiff — being stood on is a fast event — and explicit
//! integration of a stiff spring either needs a timestep nobody can afford or
//! quietly explodes. Backward Euler with a handful of weighted Jacobi sweeps is
//! unconditionally stable, so the tuning values above can be whatever looks
//! right rather than whatever the integrator tolerates. The coupling is weak
//! enough that the diagonal dominates heavily and the sweeps converge in a few
//! iterations.

use bevy::prelude::*;
use rayon::prelude::*;

use crate::noise::{fbm, smoothstep_between};
use crate::params::GrassParams;
use crate::wind::WindField;

/// Cells along each edge of the default field.
pub const DEFAULT_RESOLUTION: usize = 256;

/// Metres per cell.
///
/// A cell is a small clump of grass, not a blade. Finer than this buys nothing:
/// bending is a soft, spatially smooth effect, and the renderer interpolates
/// between cells anyway.
pub const DEFAULT_CELL_SIZE: f32 = 0.15;

/// The fixed simulation step, in seconds.
///
/// Fixed rather than frame-coupled so that the same shove produces the same
/// motion on a 60 Hz laptop and a 240 Hz monitor. A spring integrated at a
/// varying timestep changes its apparent stiffness with frame rate.
pub const SIM_STEP: f32 = 1.0 / 60.0;

/// Most fixed steps run for one frame.
///
/// After a hitch, catching up fully would cost more than the hitch did and
/// cause another. Dropping the backlog loses a little simulated time, which
/// nobody can see, instead of stuttering, which everybody can.
const MAX_STEPS_PER_FRAME: u32 = 3;

/// Wind is evaluated on a grid this many times coarser than the field.
///
/// Wind is coherent over metres while cells are centimetres, so evaluating it
/// per cell is the same answer computed sixteen times. This one constant is the
/// difference between wind costing a fifth of a millisecond and costing three.
const WIND_DOWNSAMPLE: usize = 4;

/// Weighted Jacobi sweeps per step.
const JACOBI_ITERATIONS: usize = 6;

/// Under-relaxation, which converges faster than plain Jacobi.
const JACOBI_RELAXATION: f32 = 0.75;

/// Rows of the field batched into one parallel task.
///
/// A step dispatches eight parallel loops, so handing out one task per row
/// costs more in scheduling than the row costs to compute — which is why a
/// small field was measurably *slower* threaded than serial before this.
const ROWS_PER_TASK: usize = 8;

/// The bend field.
#[derive(Resource, Clone, Debug)]
pub struct GrassField {
    resolution: usize,
    cell_size: f32,
    /// World position of the field's minimum corner.
    origin: Vec2,
    params: GrassParams,

    // Dynamic state, one entry per cell.
    theta: Vec<Vec2>,
    omega: Vec<Vec2>,
    fast_memory: Vec<Vec2>,
    slow_memory: Vec<Vec2>,
    axis: Vec<Vec2>,
    compaction: Vec<f32>,
    dose: Vec<f32>,

    // Terrain properties, fixed at construction.
    density: Vec<f32>,
    length: Vec<f32>,
    stiffness: Vec<f32>,

    // Derived from the three above, so recomputed only when they change rather
    // than for every cell on every step. These involve a square root and two
    // divisions each, and at a quarter of a million cells sixty times a second
    // that is a millisecond of doing the same arithmetic over and over.
    natural: Vec<f32>,
    structural: Vec<f32>,
    base_damping: Vec<f32>,

    // Contact accumulators, cleared every step.
    contact_polar: Vec<Vec2>,
    contact_axis: Vec<Vec2>,
    contact_weight: Vec<f32>,
    contact_severity: Vec<f32>,
    impulse: Vec<Vec2>,

    // Solver scratch, kept allocated across steps.
    diagonal: Vec<f32>,
    rhs: Vec<Vec2>,
    coupling_x: Vec<f32>,
    coupling_y: Vec<f32>,
    solve: Vec<Vec2>,
    solve_next: Vec<Vec2>,
    wind_coarse: Vec<Vec2>,
    wind_resolution: usize,

    leftover_time: f32,
    steps_taken: u64,
}

impl Default for GrassField {
    fn default() -> Self {
        Self::new(DEFAULT_RESOLUTION, DEFAULT_CELL_SIZE, 0xB1AD_E5EE)
    }
}

impl GrassField {
    /// A field of the given size, centred on the world origin.
    pub fn new(resolution: usize, cell_size: f32, seed: u32) -> Self {
        let resolution = resolution.max(2);
        let cells = resolution * resolution;
        let extent = resolution as f32 * cell_size;
        let wind_resolution = resolution / WIND_DOWNSAMPLE + 2;

        let mut field = Self {
            resolution,
            cell_size,
            origin: Vec2::splat(-extent * 0.5),
            params: GrassParams::default(),

            theta: vec![Vec2::ZERO; cells],
            omega: vec![Vec2::ZERO; cells],
            fast_memory: vec![Vec2::ZERO; cells],
            slow_memory: vec![Vec2::ZERO; cells],
            axis: vec![Vec2::ZERO; cells],
            compaction: vec![0.0; cells],
            dose: vec![0.0; cells],

            density: vec![1.0; cells],
            length: vec![0.24; cells],
            stiffness: vec![1.0; cells],

            natural: vec![0.0; cells],
            structural: vec![0.0; cells],
            base_damping: vec![0.0; cells],

            contact_polar: vec![Vec2::ZERO; cells],
            contact_axis: vec![Vec2::ZERO; cells],
            contact_weight: vec![0.0; cells],
            contact_severity: vec![0.0; cells],
            impulse: vec![Vec2::ZERO; cells],

            diagonal: vec![1.0; cells],
            rhs: vec![Vec2::ZERO; cells],
            coupling_x: vec![0.0; cells],
            coupling_y: vec![0.0; cells],
            solve: vec![Vec2::ZERO; cells],
            solve_next: vec![Vec2::ZERO; cells],
            wind_coarse: vec![Vec2::ZERO; wind_resolution * wind_resolution],
            wind_resolution,

            leftover_time: 0.0,
            steps_taken: 0,
        };
        field.generate_terrain(seed);
        field
    }

    /// Give every cell smoothly varying grass properties.
    ///
    /// Smooth rather than per-cell random on purpose. Independent neighbours
    /// produce visual static — grass that shimmers because each clump has an
    /// unrelated frequency — whereas correlated variation reads as one meadow
    /// with patchy ground under it.
    fn generate_terrain(&mut self, seed: u32) {
        for y in 0..self.resolution {
            for x in 0..self.resolution {
                let index = y * self.resolution + x;
                let world = self.cell_center(x, y);
                // Metres, so the patch scale does not change with resolution.
                let coarse = world * 0.09;
                let fine = world * 0.31;

                // Thinner in places, but never bald. Meadow grass is dense
                // nearly everywhere, and a density map that reaches zero puts
                // dark holes through the canopy that read as damage rather than
                // as variation. Bare ground is something terrain should ask for
                // explicitly, not something noise produces by accident.
                let patchiness = fbm(coarse.x, coarse.y, seed, 3);
                self.density[index] = 0.62 + 0.38 * patchiness;
                self.length[index] =
                    0.21 + 0.20 * fbm(fine.x + 31.0, fine.y - 17.0, seed ^ 0x51ED, 2);
                self.stiffness[index] =
                    0.72 + 0.56 * fbm(fine.x - 9.0, fine.y + 44.0, seed ^ 0xA113, 2);
            }
        }
        self.refresh_constants();
    }

    /// Recompute everything derived from the terrain properties.
    ///
    /// Must be called after any change to density, length or stiffness.
    fn refresh_constants(&mut self) {
        let p = self.params;
        let tau = std::f32::consts::TAU;
        for index in 0..self.natural.len() {
            // Cantilever scaling: frequency goes as sqrt(EI) / L^2. Short grass
            // buzzes and long grass wallows, and both fall out of this one
            // relation rather than needing to be authored.
            let frequency = (p.natural_frequency
                * self.stiffness[index].sqrt()
                * (p.reference_length / self.length[index].max(1e-3)).powi(2))
            .clamp(p.frequency_range.0, p.frequency_range.1);
            let natural = tau * frequency;
            self.natural[index] = natural;
            self.structural[index] = natural * natural;
            // The damping of undisturbed grass. Compaction adds to it, but
            // only where something has actually been crushed.
            let ratio = (p.damping_ratio + p.density_damping * self.density[index])
                .clamp(p.damping_range.0, p.damping_range.1);
            self.base_damping[index] = 2.0 * ratio * natural;
        }
    }

    // --- geometry ------------------------------------------------------

    pub fn resolution(&self) -> usize {
        self.resolution
    }

    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }

    pub fn origin(&self) -> Vec2 {
        self.origin
    }

    /// Width of the field in metres.
    pub fn extent(&self) -> f32 {
        self.resolution as f32 * self.cell_size
    }

    pub fn params(&self) -> &GrassParams {
        &self.params
    }

    pub fn params_mut(&mut self) -> &mut GrassParams {
        &mut self.params
    }

    pub fn steps_taken(&self) -> u64 {
        self.steps_taken
    }

    /// World position of a cell's centre.
    pub fn cell_center(&self, x: usize, y: usize) -> Vec2 {
        self.origin + Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * self.cell_size
    }

    /// The inclusive cell range covering a world-space box, clipped to the
    /// field. Returns `None` when the box misses the field entirely.
    pub fn cell_range(&self, min: Vec2, max: Vec2) -> Option<(usize, usize, usize, usize)> {
        let last = self.resolution as f32 - 1.0;
        let to_cell = |v: Vec2| (v - self.origin) / self.cell_size - Vec2::splat(0.5);
        let low = to_cell(min);
        let high = to_cell(max);
        if high.x < 0.0 || high.y < 0.0 || low.x > last || low.y > last {
            return None;
        }
        Some((
            low.x.floor().clamp(0.0, last) as usize,
            low.y.floor().clamp(0.0, last) as usize,
            high.x.ceil().clamp(0.0, last) as usize,
            high.y.ceil().clamp(0.0, last) as usize,
        ))
    }

    fn index(&self, x: usize, y: usize) -> usize {
        y * self.resolution + x
    }

    /// The cell containing a world position, if it is inside the field.
    pub fn cell_at(&self, world: Vec2) -> Option<(usize, usize)> {
        let local = (world - self.origin) / self.cell_size;
        if local.x < 0.0 || local.y < 0.0 {
            return None;
        }
        let (x, y) = (local.x as usize, local.y as usize);
        if x >= self.resolution || y >= self.resolution {
            return None;
        }
        Some((x, y))
    }

    // --- reading -------------------------------------------------------

    pub fn theta(&self) -> &[Vec2] {
        &self.theta
    }

    pub fn axis(&self) -> &[Vec2] {
        &self.axis
    }

    pub fn compaction(&self) -> &[f32] {
        &self.compaction
    }

    pub fn density(&self) -> &[f32] {
        &self.density
    }

    pub fn length(&self) -> &[f32] {
        &self.length
    }

    /// Bilinearly sampled bend at a world position.
    ///
    /// Outside the field the grass is upright, not an error: chunk edges and
    /// stray interactors routinely sample past the boundary.
    pub fn bend_at(&self, world: Vec2) -> Vec2 {
        self.sample_vec2(&self.theta, world)
    }

    /// Bilinearly sampled compaction at a world position.
    pub fn compaction_at(&self, world: Vec2) -> f32 {
        self.sample_f32(&self.compaction, world)
    }

    /// Bilinearly sampled flattening axis at a world position.
    pub fn axis_at(&self, world: Vec2) -> Vec2 {
        self.sample_vec2(&self.axis, world)
    }

    /// Bilinearly sampled slow directional memory at a world position.
    ///
    /// The *signed* record of which way grass was pushed, as opposed to
    /// [`axis_at`](Self::axis_at), which records the unsigned axis. Comparing
    /// the two is what tells a one-way trail apart from a two-way path.
    pub fn slow_memory_at(&self, world: Vec2) -> Vec2 {
        self.sample_vec2(&self.slow_memory, world)
    }

    /// Bilinearly sampled contact dose at a world position, in severity-seconds.
    ///
    /// The channel that responds the instant a cell is touched, which makes it
    /// the right thing to measure when asking whether a stamp covered ground —
    /// compaction deliberately lags, so it answers a different question.
    pub fn dose_at(&self, world: Vec2) -> f32 {
        self.sample_f32(&self.dose, world)
    }

    fn sample_coords(&self, world: Vec2) -> Option<(usize, usize, usize, usize, f32, f32)> {
        let last = self.resolution - 1;
        let local = (world - self.origin) / self.cell_size - Vec2::splat(0.5);
        if local.x < -1.0 || local.y < -1.0 {
            return None;
        }
        let x0 = local.x.floor();
        let y0 = local.y.floor();
        if x0 > last as f32 || y0 > last as f32 {
            return None;
        }
        let fx = local.x - x0;
        let fy = local.y - y0;
        let ix = x0.max(0.0) as usize;
        let iy = y0.max(0.0) as usize;
        Some((ix, iy, (ix + 1).min(last), (iy + 1).min(last), fx, fy))
    }

    fn sample_vec2(&self, data: &[Vec2], world: Vec2) -> Vec2 {
        let Some((x0, y0, x1, y1, fx, fy)) = self.sample_coords(world) else {
            return Vec2::ZERO;
        };
        let bottom = data[self.index(x0, y0)].lerp(data[self.index(x1, y0)], fx);
        let top = data[self.index(x0, y1)].lerp(data[self.index(x1, y1)], fx);
        bottom.lerp(top, fy)
    }

    fn sample_f32(&self, data: &[f32], world: Vec2) -> f32 {
        let Some((x0, y0, x1, y1, fx, fy)) = self.sample_coords(world) else {
            return 0.0;
        };
        let bottom = lerp(data[self.index(x0, y0)], data[self.index(x1, y0)], fx);
        let top = lerp(data[self.index(x0, y1)], data[self.index(x1, y1)], fx);
        lerp(bottom, top, fy)
    }

    // --- writing -------------------------------------------------------

    /// Add one contact sample to a cell.
    ///
    /// Contributions accumulate additively and are averaged by total weight at
    /// solve time, so two units standing in one cell agree on a direction
    /// rather than each overwriting the other.
    pub fn accumulate_contact(
        &mut self,
        x: usize,
        y: usize,
        target_angle: Vec2,
        direction: Vec2,
        weight: f32,
        severity_rate: f32,
    ) {
        let index = self.index(x, y);
        self.contact_polar[index] += target_angle * weight;
        // The outer product in double-angle form: opposite directions add, and
        // perpendicular ones cancel. See the module docs.
        self.contact_axis[index] += Vec2::new(
            direction.x * direction.x - direction.y * direction.y,
            2.0 * direction.x * direction.y,
        ) * weight;
        self.contact_weight[index] += weight;
        self.contact_severity[index] += severity_rate;
    }

    /// Kick a cell's angular velocity directly.
    ///
    /// For events too brief to resolve as a sustained contact — an explosion is
    /// over before the next step. Applying it as an impulse rather than a
    /// target angle means the grass is *thrown* and then recovers under its own
    /// dynamics, which is what makes a blast look like a blast.
    pub fn add_impulse(&mut self, x: usize, y: usize, impulse: Vec2) {
        let index = self.index(x, y);
        self.impulse[index] += impulse;
    }

    /// Density of a cell, in 0..=1.
    pub fn density_at_cell(&self, x: usize, y: usize) -> f32 {
        self.density[self.index(x, y)]
    }

    /// Bilinearly sampled density at a world position, in 0..=1.
    ///
    /// Sampled rather than nearest so blade placement thins out smoothly across
    /// the edge of a bare patch. Nearest would put a visible cell-shaped step
    /// there, and a grid of them is instantly readable as a grid.
    pub fn density_at_world(&self, world: Vec2) -> f32 {
        self.sample_f32(&self.density, world)
    }

    /// Bilinearly sampled blade length at a world position, in metres.
    pub fn length_at_world(&self, world: Vec2) -> f32 {
        self.sample_f32(&self.length, world)
    }

    /// Overwrite density everywhere. For tests and for bare-ground scenarios.
    pub fn set_density_everywhere(&mut self, density: f32) {
        self.density.fill(density.clamp(0.0, 1.0));
        self.refresh_constants();
    }

    // --- stepping ------------------------------------------------------

    /// Run however many fixed steps `delta_seconds` has earned.
    pub fn advance(&mut self, delta_seconds: f32, wind: &WindField) {
        self.leftover_time += delta_seconds.clamp(0.0, 1.0);
        let mut steps = 0;
        while self.leftover_time >= SIM_STEP && steps < MAX_STEPS_PER_FRAME {
            self.step(SIM_STEP, wind);
            self.leftover_time -= SIM_STEP;
            steps += 1;
        }
        if self.leftover_time > SIM_STEP {
            self.leftover_time = 0.0;
        }
    }

    /// One fixed step.
    pub fn step(&mut self, dt: f32, wind: &WindField) {
        if dt <= 0.0 {
            return;
        }
        self.bake_wind(wind);
        self.build_system(dt, wind);
        self.build_coupling();
        self.solve_jacobi();
        self.finalise(dt);
        self.clear_accumulators();
        self.steps_taken += 1;
    }

    /// Evaluate wind onto its coarse lattice.
    fn bake_wind(&mut self, wind: &WindField) {
        let spacing = WIND_DOWNSAMPLE as f32 * self.cell_size;
        for j in 0..self.wind_resolution {
            for i in 0..self.wind_resolution {
                let world = self.origin + Vec2::new(i as f32, j as f32) * spacing;
                self.wind_coarse[j * self.wind_resolution + i] = wind.velocity_at(world);
            }
        }
    }

    /// Build the per-cell diagonal and right-hand side of the implicit system.
    fn build_system(&mut self, dt: f32, wind: &WindField) {
        let p = self.params;
        let inverse_dt = 1.0 / dt;
        let inverse_dt2 = inverse_dt * inverse_dt;
        let tau = std::f32::consts::TAU;
        let wind_frequency_squared = (tau * p.wind_frequency).powi(2);

        let resolution = self.resolution;
        let wind_resolution = self.wind_resolution;

        // Destructured so the borrow checker can see that the three buffers
        // being written are different fields from the dozen being read.
        let Self {
            diagonal,
            rhs,
            solve,
            theta: theta_all,
            omega: omega_all,
            impulse,
            density: density_all,
            length: length_all,
            stiffness: stiffness_all,
            natural: natural_all,
            structural: structural_all,
            base_damping,
            compaction: compaction_all,
            contact_weight,
            contact_polar,
            fast_memory,
            slow_memory,
            wind_coarse,
            ..
        } = self;

        diagonal
            .par_chunks_mut(resolution)
            .zip(rhs.par_chunks_mut(resolution))
            .zip(solve.par_chunks_mut(resolution))
            .with_min_len(ROWS_PER_TASK)
            .enumerate()
            .for_each(|(y, ((diagonal_row, rhs_row), solve_row))| {
                for x in 0..resolution {
                    let index = y * resolution + x;
                    let density = density_all[index];
                    if density <= 0.0 {
                        diagonal_row[x] = 1.0;
                        rhs_row[x] = Vec2::ZERO;
                        solve_row[x] = Vec2::ZERO;
                        continue;
                    }

                    let theta = theta_all[index];
                    // Impulses join the velocity *before* the solve, not after it.
                    // Applying them afterwards costs a step of latency, so a blast
                    // would visibly lag the frame it went off in.
                    let omega = omega_all[index] + impulse[index];
                    let length = length_all[index];
                    let stiffness = stiffness_all[index];

                    let natural = natural_all[index];
                    let structural = structural_all[index];
                    let mut structural_damping = base_damping[index];
                    let compaction = compaction_all[index];
                    if compaction > 0.0 {
                        // Crushed grass is tangled and moves less freely. Only
                        // worth the extra arithmetic where something has actually
                        // crushed it, which is a tiny part of any field.
                        let ratio = (p.damping_ratio
                            + p.density_damping * density
                            + p.compaction_damping * compaction)
                            .clamp(p.damping_range.0, p.damping_range.1);
                        structural_damping = 2.0 * ratio * natural;
                    }

                    // Contact.
                    let weight = contact_weight[index];
                    let (contact_target, contact_stiffness, contact_damping) = if weight > 1e-6 {
                        let strength = 1.0 - (-weight).exp();
                        let frequency =
                            lerp(p.contact_frequency.0, p.contact_frequency.1, strength);
                        let stiffness = (tau * frequency).powi(2) * strength;
                        (
                            soft_cap(contact_polar[index] / weight, p.max_angle),
                            stiffness,
                            2.0 * p.contact_damping * stiffness.max(0.0).sqrt(),
                        )
                    } else {
                        (Vec2::ZERO, 0.0, 0.0)
                    };

                    // Wind.
                    let response = wind.lean_target(
                        sample_coarse_wind(wind_coarse, wind_resolution, x, y),
                        length,
                        stiffness,
                        theta,
                    );
                    let (wind_stiffness, wind_damping) = if response.strength > 1e-6 {
                        let stiffness = wind_frequency_squared * response.strength;
                        (stiffness, 2.0 * p.wind_damping * stiffness.max(0.0).sqrt())
                    } else {
                        (0.0, 0.0)
                    };

                    let nonlinear = p.high_angle_stiffness * theta.length_squared();
                    let permanent = structural * p.permanent_fraction;
                    let fast = structural * p.fast_fraction;
                    let slow = structural * p.slow_fraction;

                    let total_stiffness =
                        permanent + fast + slow + nonlinear + contact_stiffness + wind_stiffness;
                    let total_damping = structural_damping + contact_damping + wind_damping;

                    diagonal_row[x] = inverse_dt2 + total_damping * inverse_dt + total_stiffness;
                    rhs_row[x] = (theta + omega * dt) * inverse_dt2
                        + theta * (total_damping * inverse_dt)
                        + fast_memory[index] * fast
                        + slow_memory[index] * slow
                        + contact_target * contact_stiffness
                        + response.target * wind_stiffness;

                    // Start from where the grass already is; one step of motion is
                    // a small correction, so this is close to the answer.
                    solve_row[x] = theta;
                }
            });
    }

    /// Precompute per-edge coupling coefficients.
    ///
    /// Once per step rather than once per Jacobi sweep, because they depend
    /// only on the previous state. Six sweeps would otherwise recompute the
    /// same numbers six times.
    fn build_coupling(&mut self) {
        let p = self.params;
        // The correlation length relates coupling to structural stiffness by
        // l^2 = kappa / k, and the 1/h^2 of the Laplacian cancels the h^2 in
        // expressing that length in metres. What survives is dimensionless.
        let scale = p.correlation_cells * p.correlation_cells;
        let falloff = (p.coupling_falloff * p.coupling_falloff).max(1e-6);
        let resolution = self.resolution;

        let Self {
            coupling_x,
            coupling_y,
            density,
            theta,
            structural,
            ..
        } = self;

        let edge = |a: usize, b: usize| -> f32 {
            let joint = density[a] * density[b];
            if joint <= 0.0 {
                return 0.0;
            }
            // Edge-aware: strongly differing neighbours barely pull on each
            // other, so a flattened track keeps its edge instead of smearing
            // into the upright grass beside it.
            let difference = (theta[a] - theta[b]).length_squared();
            let similarity = 1.0 / (1.0 + difference / falloff);
            let stiffness = 0.5 * (structural[a] + structural[b]);
            stiffness * scale * joint.sqrt() * similarity
        };

        coupling_x
            .par_chunks_mut(resolution)
            .zip(coupling_y.par_chunks_mut(resolution))
            .with_min_len(ROWS_PER_TASK)
            .enumerate()
            .for_each(|(y, (row_x, row_y))| {
                for x in 0..resolution {
                    let index = y * resolution + x;
                    row_x[x] = if x + 1 < resolution {
                        edge(index, index + 1)
                    } else {
                        0.0
                    };
                    row_y[x] = if y + 1 < resolution {
                        edge(index, index + resolution)
                    } else {
                        0.0
                    };
                }
            });
    }

    /// Weighted Jacobi sweeps.
    ///
    /// Threaded by row. Jacobi reads the previous iterate and writes a separate
    /// buffer, so no two threads ever look at the same value one of them is
    /// changing — the result is identical to the serial version, not merely
    /// close to it. (Gauss-Seidel would converge in fewer sweeps but reads its
    /// own output, which is exactly what cannot be threaded this way.)
    fn solve_jacobi(&mut self) {
        let resolution = self.resolution;
        for _ in 0..JACOBI_ITERATIONS {
            {
                // Destructured so the borrow checker can see that the buffer
                // being written is a different field from the ones being read.
                let Self {
                    solve,
                    solve_next,
                    coupling_x,
                    coupling_y,
                    rhs,
                    diagonal,
                    density,
                    ..
                } = self;

                solve_next
                    .par_chunks_mut(resolution)
                    .with_min_len(ROWS_PER_TASK)
                    .enumerate()
                    .for_each(|(y, row)| {
                        for (x, out) in row.iter_mut().enumerate() {
                            let index = y * resolution + x;
                            if density[index] <= 0.0 {
                                *out = Vec2::ZERO;
                                continue;
                            }

                            let mut neighbours = Vec2::ZERO;
                            let mut neighbour_diagonal = 0.0;
                            if x > 0 {
                                let coefficient = coupling_x[index - 1];
                                neighbours += solve[index - 1] * coefficient;
                                neighbour_diagonal += coefficient;
                            }
                            if x + 1 < resolution {
                                let coefficient = coupling_x[index];
                                neighbours += solve[index + 1] * coefficient;
                                neighbour_diagonal += coefficient;
                            }
                            if y > 0 {
                                let coefficient = coupling_y[index - resolution];
                                neighbours += solve[index - resolution] * coefficient;
                                neighbour_diagonal += coefficient;
                            }
                            if y + 1 < resolution {
                                let coefficient = coupling_y[index];
                                neighbours += solve[index + resolution] * coefficient;
                                neighbour_diagonal += coefficient;
                            }

                            let candidate =
                                (rhs[index] + neighbours) / (diagonal[index] + neighbour_diagonal);
                            *out = solve[index].lerp(candidate, JACOBI_RELAXATION);
                        }
                    });
            }
            std::mem::swap(&mut self.solve, &mut self.solve_next);
        }
    }

    /// Commit the solved angles and advance every memory channel.
    fn finalise(&mut self, dt: f32) {
        let p = self.params;
        for index in 0..self.theta.len() {
            if self.density[index] <= 0.0 {
                continue;
            }

            let previous = self.theta[index];
            let solved = soft_cap(self.solve[index], p.max_angle);
            self.theta[index] = solved;
            // Any impulse is already folded into the solve, so the resulting
            // velocity is simply how far the grass actually moved.
            self.omega[index] = (solved - previous) / dt;

            // Everything below is the memory machinery: seven exponentials per
            // cell, for grass that remembers being trodden on. The overwhelming
            // majority of a field has never been touched and has nothing to
            // remember, and running it anyway was most of the cost of a step.
            if self.contact_severity[index] <= 0.0
                && self.dose[index] <= 0.0
                && self.compaction[index] <= 0.0
                && self.fast_memory[index] == Vec2::ZERO
                && self.slow_memory[index] == Vec2::ZERO
                && self.axis[index] == Vec2::ZERO
            {
                continue;
            }

            let weight = self.contact_weight[index];
            // How hard this cell is being contacted *right now*, in 0..=1, and
            // deliberately independent of the timestep. Folding `dt` in here
            // would make severity mean "how much contact happened this step",
            // which at sixty steps a second is always a tiny number — and every
            // activation threshold downstream would then be silently tied to
            // the frame rate.
            let severity = 1.0 - (-self.contact_severity[index]).exp();
            let contact_axis = if weight > 1e-6 {
                self.contact_axis[index] / weight
            } else {
                Vec2::ZERO
            };

            // Dose is a leaky integral of severity: a hundred footfalls in one
            // place add up, and a single one fades. This is where `dt` belongs,
            // and only here.
            self.dose[index] = self.dose[index] * (-dt / p.dose_decay).exp() + severity * dt;

            let fast_activation =
                smoothstep_between(p.fast_activation.0, p.fast_activation.1, severity);
            let slow_activation =
                smoothstep_between(p.slow_activation.0, p.slow_activation.1, self.dose[index]);

            self.fast_memory[index] = relax_vec2(
                self.fast_memory[index],
                solved,
                fast_activation,
                dt,
                p.fast_set,
                p.fast_recover,
            );
            self.slow_memory[index] = relax_vec2(
                self.slow_memory[index],
                solved,
                slow_activation,
                dt,
                p.slow_set,
                p.slow_recover,
            );

            let desired = 1.0 - (-p.dose_to_compaction * self.dose[index]).exp();
            self.compaction[index] = relax_f32(
                self.compaction[index],
                desired,
                slow_activation,
                dt,
                p.compaction_set,
                p.compaction_recover,
            );
            self.axis[index] = relax_vec2(
                self.axis[index],
                contact_axis * desired,
                slow_activation,
                dt,
                p.axis_set,
                p.axis_recover,
            );

            // Alignment can never exceed how crushed the grass is: an axis
            // stronger than its compaction would render as blades laid flat in
            // a patch that is standing up.
            let alignment = self.axis[index].length();
            if alignment > self.compaction[index] {
                self.axis[index] *= self.compaction[index] / alignment.max(1e-6);
            }

            // Snap what has faded to nothing all the way to zero. Exponential
            // decay never actually arrives, and without this a cell trodden on
            // once stays on the expensive path forever — which over a long
            // battle is every cell.
            settle(&mut self.fast_memory[index]);
            settle(&mut self.slow_memory[index]);
            settle(&mut self.axis[index]);
            if self.compaction[index] < QUIET {
                self.compaction[index] = 0.0;
            }
            if self.dose[index] < QUIET {
                self.dose[index] = 0.0;
            }
        }
    }

    fn clear_accumulators(&mut self) {
        self.contact_polar.fill(Vec2::ZERO);
        self.contact_axis.fill(Vec2::ZERO);
        self.contact_weight.fill(0.0);
        self.contact_severity.fill(0.0);
        self.impulse.fill(Vec2::ZERO);
    }

    // --- diagnostics ---------------------------------------------------

    /// Total mechanical energy: kinetic plus elastic.
    ///
    /// Only meaningful with no forcing, where it must fall monotonically. An
    /// energy that climbs is the signature of an unstable integrator, and it is
    /// far easier to catch here than by watching for grass to start vibrating.
    pub fn energy(&self) -> f64 {
        let mut total = 0.0;
        for index in 0..self.theta.len() {
            let kinetic = 0.5 * self.omega[index].length_squared();
            let elastic = 0.5 * self.structural[index] * self.theta[index].length_squared();
            total += (kinetic + elastic) as f64;
        }

        // Energy held in the springs *between* cells has to be counted too.
        // Leave it out and the total appears to rise whenever a disturbance is
        // in transit from one cell to its neighbour — which is indistinguishable
        // from the instability this function exists to detect.
        for y in 0..self.resolution {
            for x in 0..self.resolution {
                let index = self.index(x, y);
                if x + 1 < self.resolution {
                    let difference = (self.theta[index] - self.theta[index + 1]).length_squared();
                    total += (0.5 * self.coupling_x[index] * difference) as f64;
                }
                if y + 1 < self.resolution {
                    let below = index + self.resolution;
                    let difference = (self.theta[index] - self.theta[below]).length_squared();
                    total += (0.5 * self.coupling_y[index] * difference) as f64;
                }
            }
        }
        total
    }

    /// Bytes of per-cell state the field holds.
    pub fn byte_size(&self) -> usize {
        let cells = self.theta.len();
        let vectors = 11; // theta, omega, two memories, axis, accumulators, scratch
        let scalars = 8; // compaction, dose, density, length, stiffness, structural, diagonal, weight
        cells * (vectors * size_of::<Vec2>() + scalars * size_of::<f32>())
            + self.wind_coarse.len() * size_of::<Vec2>()
    }

    /// Bytes uploaded to the GPU each frame.
    pub fn upload_bytes(&self) -> usize {
        // One RGBA32F bend texture and one R32F state texture.
        self.theta.len() * (4 + 1) * size_of::<f32>()
    }

    /// Largest bend anywhere, in radians.
    pub fn max_bend(&self) -> f32 {
        self.theta.iter().map(|t| t.length()).fold(0.0, f32::max)
    }

    /// Mean bend magnitude over grassed cells, in radians.
    pub fn mean_bend(&self) -> f32 {
        let mut total = 0.0;
        let mut count = 0;
        for index in 0..self.theta.len() {
            if self.density[index] > 0.0 {
                total += self.theta[index].length();
                count += 1;
            }
        }
        if count == 0 {
            0.0
        } else {
            total / count as f32
        }
    }

    /// Mean compaction over grassed cells.
    pub fn mean_compaction(&self) -> f32 {
        let mut total = 0.0;
        let mut count = 0;
        for index in 0..self.compaction.len() {
            if self.density[index] > 0.0 {
                total += self.compaction[index];
                count += 1;
            }
        }
        if count == 0 {
            0.0
        } else {
            total / count as f32
        }
    }

    /// Reset all dynamic state, keeping terrain properties.
    pub fn reset(&mut self) {
        self.theta.fill(Vec2::ZERO);
        self.omega.fill(Vec2::ZERO);
        self.fast_memory.fill(Vec2::ZERO);
        self.slow_memory.fill(Vec2::ZERO);
        self.axis.fill(Vec2::ZERO);
        self.compaction.fill(0.0);
        self.dose.fill(0.0);
        self.clear_accumulators();
        self.leftover_time = 0.0;
        self.steps_taken = 0;
    }

    /// Make every cell fully grassed and uniform.
    ///
    /// For tests and benchmarks, where generated patchiness would make one seed
    /// measure something slightly different from the next.
    pub fn make_uniform(&mut self, length: f32, stiffness: f32) {
        self.density.fill(1.0);
        self.length.fill(length);
        self.stiffness.fill(stiffness);
        self.refresh_constants();
    }
}

/// Squash a vector's length toward a ceiling it never quite reaches.
///
/// `tanh` rather than a hard clamp: a clamp reads as the blade hitting an
/// invisible wall, and it also puts a kink in the solve that the Jacobi sweeps
/// then argue with.
/// Below this, a memory is indistinguishable from having none.
///
/// Comfortably under a tenth of a degree of bend, and under a thousandth of
/// full compaction.
const QUIET: f32 = 1.0e-4;

/// Bilinear wind velocity for a cell, from the coarse lattice.
fn sample_coarse_wind(coarse: &[Vec2], resolution: usize, x: usize, y: usize) -> Vec2 {
    let scale = 1.0 / WIND_DOWNSAMPLE as f32;
    let fx = (x as f32 + 0.5) * scale;
    let fy = (y as f32 + 0.5) * scale;
    let last = resolution - 1;
    let x0 = (fx.floor() as usize).min(last);
    let y0 = (fy.floor() as usize).min(last);
    let x1 = (x0 + 1).min(last);
    let y1 = (y0 + 1).min(last);
    let tx = fx - fx.floor();
    let ty = fy - fy.floor();

    let bottom = coarse[y0 * resolution + x0].lerp(coarse[y0 * resolution + x1], tx);
    let top = coarse[y1 * resolution + x0].lerp(coarse[y1 * resolution + x1], tx);
    bottom.lerp(top, ty)
}

/// Zero a memory that has faded past the point of being visible.
fn settle(value: &mut Vec2) {
    if value.length_squared() < QUIET * QUIET {
        *value = Vec2::ZERO;
    }
}

fn soft_cap(v: Vec2, maximum: f32) -> Vec2 {
    let length = v.length();
    if length <= 1e-6 || maximum <= 0.0 {
        return v;
    }
    v * (maximum * (length / maximum).tanh() / length)
}

/// Advance a memory channel toward a target under two competing rates.
///
/// Solved exactly rather than stepped. The recovery constants here run to tens
/// of seconds while the step is sixteen milliseconds, and explicit integration
/// of a rate that slow either drifts or, with a large enough step, overshoots
/// past the target and oscillates. The closed form is stable at any step and no
/// more expensive.
fn relax_vec2(
    current: Vec2,
    target: Vec2,
    activation: f32,
    dt: f32,
    set_time: f32,
    recover_time: f32,
) -> Vec2 {
    let (equilibrium, decay) = relax_terms(activation, dt, set_time, recover_time);
    let settled = target * equilibrium;
    settled + (current - settled) * decay
}

fn relax_f32(
    current: f32,
    target: f32,
    activation: f32,
    dt: f32,
    set_time: f32,
    recover_time: f32,
) -> f32 {
    let (equilibrium, decay) = relax_terms(activation, dt, set_time, recover_time);
    let settled = target * equilibrium;
    settled + (current - settled) * decay
}

/// Shared algebra: `dp/dt = set (target - p) - recover p` has equilibrium
/// `target * set / (set + recover)` and relaxes at `set + recover`.
fn relax_terms(activation: f32, dt: f32, set_time: f32, recover_time: f32) -> (f32, f32) {
    let activation = activation.clamp(0.0, 1.0);
    let set_rate = activation / set_time.max(1e-4);
    let recover_rate = (1.0 - activation) / recover_time.max(1e-4);
    let total = set_rate + recover_rate;
    if total <= 1e-9 {
        return (0.0, 1.0);
    }
    (set_rate / total, (-total * dt).exp())
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Step the field each frame.
pub fn step_field(time: Res<Time>, wind: Res<WindField>, mut field: ResMut<GrassField>) {
    field.advance(time.delta_secs(), &wind);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dead calm, so a test measures the thing it is about.
    pub(crate) fn calm() -> WindField {
        WindField {
            speed: 0.0,
            turbulence: 0.0,
            gust_strength: 0.0,
            ..Default::default()
        }
    }

    fn uniform(resolution: usize) -> GrassField {
        let mut field = GrassField::new(resolution, DEFAULT_CELL_SIZE, 11);
        field.make_uniform(0.24, 1.0);
        field
    }

    /// Kick one cell and run for `seconds`, returning the peak bend reached.
    fn peak_after_impulse(field: &mut GrassField, impulse: Vec2, dt: f32, seconds: f32) -> f32 {
        let (x, y) = field
            .cell_at(Vec2::ZERO)
            .expect("origin is inside the field");
        field.add_impulse(x, y, impulse);
        let steps = (seconds / dt).round() as u32;
        let mut peak: f32 = 0.0;
        for _ in 0..steps {
            field.step(dt, &calm());
            peak = peak.max(field.max_bend());
        }
        peak
    }

    #[test]
    fn undisturbed_grass_stands_up() {
        let mut field = uniform(48);
        for _ in 0..120 {
            field.step(SIM_STEP, &calm());
        }
        assert_eq!(field.max_bend(), 0.0);
        assert_eq!(field.mean_compaction(), 0.0);
    }

    #[test]
    fn wind_bends_grass_downwind() {
        let mut field = uniform(48);
        let wind = WindField {
            direction: Vec2::X,
            speed: 6.0,
            turbulence: 0.0,
            gust_strength: 0.0,
            ..Default::default()
        };
        for _ in 0..180 {
            field.step(SIM_STEP, &wind);
        }
        let bend = field.bend_at(Vec2::ZERO);
        assert!(bend.x > 0.15, "expected a downwind lean, got {bend:?}");
        assert!(bend.y.abs() < bend.x * 0.2, "lean should follow the wind");
    }

    #[test]
    fn grass_springs_back_after_a_shove() {
        // The single property that separates grass from smoke: it returns.
        let mut field = uniform(48);
        let (x, y) = field.cell_at(Vec2::ZERO).unwrap();
        field.add_impulse(x, y, Vec2::X * 12.0);
        field.step(SIM_STEP, &calm());
        let shoved = field.max_bend();
        assert!(shoved > 0.1, "the shove should have bent something");

        for _ in 0..240 {
            field.step(SIM_STEP, &calm());
        }
        assert!(
            field.max_bend() < shoved * 0.1,
            "grass should have recovered: {} -> {}",
            shoved,
            field.max_bend()
        );
    }

    #[test]
    fn an_unforced_field_loses_energy() {
        // An integrator that gains energy makes grass vibrate on its own, and
        // that is far easier to catch here than by noticing it on screen weeks
        // later.
        let mut field = uniform(48);
        let (x, y) = field.cell_at(Vec2::ZERO).unwrap();
        field.add_impulse(x, y, Vec2::new(9.0, -4.0));
        field.step(SIM_STEP, &calm());

        let mut previous = field.energy();
        for step in 0..300 {
            field.step(SIM_STEP, &calm());
            let now = field.energy();
            // A whisker of tolerance: the coupling weights are computed from
            // the state at the start of a step and compared against the state
            // at the end of it, so the accounting is very slightly out of step
            // with itself. Real instability grows geometrically and blows
            // straight through this.
            assert!(
                now <= previous * 1.002 + 1e-9,
                "energy rose at step {step}: {previous} -> {now}"
            );
            previous = now;
        }
    }

    #[test]
    fn the_response_barely_changes_with_the_timestep() {
        // Backward Euler exists for this. An explicit solver at these contact
        // stiffnesses would give a different answer at every frame rate, which
        // means the grass would behave differently on different machines.
        let peaks: Vec<f32> = [1.0 / 30.0, 1.0 / 60.0, 1.0 / 120.0]
            .iter()
            .map(|&dt| peak_after_impulse(&mut uniform(48), Vec2::X * 10.0, dt, 1.0))
            .collect();

        let min = peaks.iter().cloned().fold(f32::MAX, f32::min);
        let max = peaks.iter().cloned().fold(0.0, f32::max);
        assert!(min > 0.05, "the impulse should bend the grass: {peaks:?}");
        // Backward Euler is first-order, so a coarser step damps a little more;
        // across a fourfold range of timesteps that is worth about a quarter of
        // the peak. The field always runs at `SIM_STEP` in practice, so this is
        // a check that the integrator is well behaved rather than a property
        // anything depends on — an explicit solver at these contact stiffnesses
        // would not merely differ here, it would diverge.
        assert!(
            (max - min) / max < 0.3,
            "timestep changed the response too much: {peaks:?}"
        );
    }

    #[test]
    fn the_world_direction_of_a_shove_does_not_matter() {
        // Simulating in screen space would break this, and it is the property
        // players feel without being able to name: a shove from the north
        // behaving differently from one from the east.
        let magnitudes: Vec<f32> = [Vec2::X, Vec2::Y, -Vec2::X, -Vec2::Y]
            .iter()
            .map(|&direction| {
                let mut field = uniform(48);
                peak_after_impulse(&mut field, direction * 10.0, SIM_STEP, 0.5)
            })
            .collect();

        let min = magnitudes.iter().cloned().fold(f32::MAX, f32::min);
        let max = magnitudes.iter().cloned().fold(0.0, f32::max);
        assert!(max > 0.05);
        assert!(
            (max - min) / max < 1e-3,
            "the four world directions disagreed: {magnitudes:?}"
        );
    }

    #[test]
    fn an_absurd_impulse_does_not_break_the_solver() {
        let mut field = uniform(32);
        let (x, y) = field.cell_at(Vec2::ZERO).unwrap();
        field.add_impulse(x, y, Vec2::splat(1.0e7));
        for _ in 0..120 {
            field.step(SIM_STEP, &calm());
        }
        assert!(field.max_bend().is_finite());
        assert!(
            field.max_bend() <= field.params().max_angle + 1e-3,
            "the cap was crossed: {}",
            field.max_bend()
        );
        assert!(field.theta().iter().all(|t| t.is_finite()));
    }

    #[test]
    fn bend_never_exceeds_the_cap() {
        let mut field = uniform(32);
        let wind = WindField {
            speed: 60.0,
            gust_strength: 40.0,
            ..Default::default()
        };
        for _ in 0..300 {
            field.step(SIM_STEP, &wind);
        }
        assert!(field.max_bend() <= field.params().max_angle + 1e-3);
    }

    #[test]
    fn bare_ground_never_bends() {
        let mut field = uniform(32);
        field.set_density_everywhere(0.0);
        let (x, y) = field.cell_at(Vec2::ZERO).unwrap();
        field.add_impulse(x, y, Vec2::X * 50.0);
        for _ in 0..60 {
            field.step(SIM_STEP, &WindField::default());
        }
        assert_eq!(field.max_bend(), 0.0);
    }

    #[test]
    fn coupling_stays_local() {
        // Weak coupling is what stops the field looking like rubber. If a kick
        // in one cell visibly moved grass a metre away, the whole field would
        // move as one sheet.
        let mut field = uniform(64);
        let (x, y) = field.cell_at(Vec2::ZERO).unwrap();
        field.add_impulse(x, y, Vec2::X * 14.0);

        // Peaks over the whole run, not the state at some arbitrary moment: by
        // the time the far probe would have responded, the near one has already
        // sprung back, and a snapshot would compare two different instants.
        let (mut near, mut far) = (0.0f32, 0.0f32);
        for _ in 0..90 {
            field.step(SIM_STEP, &calm());
            near = near.max(field.bend_at(Vec2::new(0.15, 0.0)).length());
            far = far.max(field.bend_at(Vec2::new(1.5, 0.0)).length());
        }
        assert!(near > 0.02, "the neighbour should feel something: {near}");
        assert!(
            far < near * 0.1,
            "the kick carried too far: near {near}, far {far}"
        );
    }

    #[test]
    fn sampling_outside_the_field_reads_as_upright() {
        let field = uniform(16);
        assert_eq!(field.bend_at(Vec2::splat(1.0e5)), Vec2::ZERO);
        assert_eq!(field.compaction_at(Vec2::splat(-1.0e5)), 0.0);
        assert_eq!(field.cell_at(Vec2::splat(1.0e5)), None);
    }

    #[test]
    fn a_variable_frame_rate_does_not_run_the_field_faster() {
        // `advance` must not let a long frame spiral: catching up fully after a
        // hitch costs more than the hitch did and causes the next one.
        let mut field = uniform(16);
        field.advance(10.0, &calm());
        assert!(
            field.steps_taken() <= MAX_STEPS_PER_FRAME as u64,
            "{} steps for one long frame",
            field.steps_taken()
        );
    }

    #[test]
    fn short_grass_moves_faster_than_long_grass() {
        // Cantilever scaling. Getting this backwards makes tall grass twitch
        // and short grass wallow, which reads as the wrong material entirely.
        // Measured as time to peak — a quarter period. Looking for the return
        // through zero instead would trip immediately, because the grass starts
        // at exactly zero.
        let time_to_peak = |length: f32| {
            let mut field = GrassField::new(24, DEFAULT_CELL_SIZE, 5);
            field.make_uniform(length, 1.0);
            let (x, y) = field.cell_at(Vec2::ZERO).unwrap();
            field.add_impulse(x, y, Vec2::X * 8.0);

            let (mut best, mut at) = (0.0f32, 0);
            for step in 1..600 {
                field.step(SIM_STEP, &calm());
                let bend = field.bend_at(Vec2::ZERO).length();
                if bend > best {
                    best = bend;
                    at = step;
                }
            }
            at
        };
        let short = time_to_peak(0.15);
        let long = time_to_peak(0.35);
        assert!(
            short < long,
            "short grass should reach its peak sooner: {short} vs {long} steps"
        );
    }

    #[test]
    fn the_terrain_generator_varies_but_never_goes_bald() {
        let field = GrassField::new(64, DEFAULT_CELL_SIZE, 7);
        let min = field.density().iter().cloned().fold(f32::MAX, f32::min);
        let max = field.density().iter().cloned().fold(0.0, f32::max);
        assert!(min > 0.4, "density dipped to {min}, which reads as damage");
        assert!(max - min > 0.05, "density is flat: {min}..{max}");

        let lengths = field.length();
        let shortest = lengths.iter().cloned().fold(f32::MAX, f32::min);
        let longest = lengths.iter().cloned().fold(0.0, f32::max);
        assert!(longest - shortest > 0.05, "blade length is flat");
    }

    #[test]
    fn a_field_is_reproducible_from_its_seed() {
        let a = GrassField::new(32, DEFAULT_CELL_SIZE, 99);
        let b = GrassField::new(32, DEFAULT_CELL_SIZE, 99);
        assert_eq!(a.density(), b.density());
        assert_eq!(a.length(), b.length());
    }
}
