//! Tuning for the bend field.
//!
//! Every value here is expressed the way someone looking at grass would
//! describe it — a sway period, a recovery half-life, a maximum angle — and
//! converted into solver coefficients at the point of use. Exposing stiffness
//! and damping coefficients directly would be a smaller struct and a much worse
//! one: nobody can tell you what `k = 246.7` looks like, but everybody can tell
//! you whether grass should stand back up in one second or thirty.
//!
//! The defaults are a starting point for mid-length grass, roughly ankle to
//! knee height. They are art direction, not measurements.

/// Tuning constants for the grass field.
#[derive(Clone, Copy, Debug)]
pub struct GrassParams {
    // --- structure -----------------------------------------------------
    /// Sway frequency of a reference-length blade, in hertz.
    ///
    /// Frequency scales as `1 / length^2` for a cantilever, so this is quoted
    /// against [`reference_length`](Self::reference_length) rather than being
    /// absolute. Short grass buzzes; long grass wallows; both fall out of the
    /// one number.
    pub natural_frequency: f32,
    /// The length the frequency above is quoted at, in metres.
    pub reference_length: f32,
    /// Frequency is clamped into this band, in hertz.
    ///
    /// The inverse-square scaling is unbounded at both ends, and a cell that
    /// happened to generate very short grass would otherwise oscillate faster
    /// than the fixed timestep can represent.
    pub frequency_range: (f32, f32),

    // --- damping -------------------------------------------------------
    /// Damping ratio of an isolated blade in still, sparse grass.
    ///
    /// Below about 0.5 the grass rings like reeds. Mid-length grass in a dense
    /// canopy is heavily damped — one visible overshoot at most.
    pub damping_ratio: f32,
    /// Extra damping from being in a dense canopy, where blades rub together.
    pub density_damping: f32,
    /// Extra damping from being crushed, where blades are tangled.
    pub compaction_damping: f32,
    /// Damping ratio is clamped into this band.
    pub damping_range: (f32, f32),

    // --- limits --------------------------------------------------------
    /// Largest bend angle, in radians. Approached smoothly, never crossed.
    pub max_angle: f32,
    /// Cubic stiffening at large angles.
    ///
    /// Real grass gets harder to bend the further over it goes. Without this
    /// the only thing stopping a blade is the angular cap, and a cap alone
    /// reads as a blade hitting an invisible wall.
    pub high_angle_stiffness: f32,

    // --- how structural stiffness is shared ----------------------------
    /// Fraction of stiffness that always pulls back toward upright.
    ///
    /// The remainder is delegated to the two memory branches. Keeping a real
    /// share here means grass whose memories have fully set still stands up
    /// eventually, rather than staying flat forever.
    pub permanent_fraction: f32,
    /// Fraction of stiffness answering to the fast memory.
    pub fast_fraction: f32,
    /// Fraction of stiffness answering to the slow memory.
    pub slow_fraction: f32,

    // --- neighbour coupling --------------------------------------------
    /// Distance over which neighbouring cells influence one another, in cells.
    ///
    /// Deliberately under one and a half. Grass clumps are not stitched
    /// together; couple them strongly and the field stops looking like grass
    /// and starts looking like rubber sheeting or water. Coherence at large
    /// scales should come from the shared wind, not from mechanical coupling.
    pub correlation_cells: f32,
    /// How sharply coupling falls off between differently bent neighbours, in
    /// radians. Stops upright grass being dragged over by a flattened track
    /// next to it, which would smear every edge in the field.
    pub coupling_falloff: f32,

    // --- contact -------------------------------------------------------
    /// Response frequency of the contact spring at zero and full contact, in
    /// hertz. Faster than the blade's own sway, because being stood on is not
    /// a suggestion.
    pub contact_frequency: (f32, f32),
    /// Damping ratio of the contact spring. At or above one, so grass being
    /// pushed does not bounce against the thing pushing it.
    pub contact_damping: f32,

    // --- wind ----------------------------------------------------------
    /// Response frequency of the wind spring, in hertz. Slower than contact:
    /// wind persuades, it does not shove.
    pub wind_frequency: f32,
    /// Damping ratio of the wind spring.
    pub wind_damping: f32,

    // --- memory --------------------------------------------------------
    /// Time constant for the fast memory to take a set, in seconds.
    pub fast_set: f32,
    /// Time constant for the fast memory to fade, in seconds.
    pub fast_recover: f32,
    /// Time constant for the slow memory to take a set, in seconds.
    pub slow_set: f32,
    /// Time constant for the slow memory to fade, in seconds.
    ///
    /// Tens of seconds. This is the number that decides how long a trail lasts.
    pub slow_recover: f32,
    /// Time constants for compaction, in seconds.
    pub compaction_set: f32,
    pub compaction_recover: f32,
    /// Time constants for the flattening axis, in seconds.
    pub axis_set: f32,
    pub axis_recover: f32,

    // --- dose ----------------------------------------------------------
    /// How long accumulated contact dose takes to fade, in seconds.
    pub dose_decay: f32,
    /// Converts accumulated dose into how crushed the grass ends up.
    pub dose_to_compaction: f32,
    /// Contact severity band over which the fast memory engages.
    pub fast_activation: (f32, f32),
    /// Accumulated-dose band over which the slow memory engages.
    ///
    /// Gated on dose rather than on instantaneous severity, so that a single
    /// footstep springs back but a hundred of them wear a path.
    pub slow_activation: (f32, f32),
}

impl Default for GrassParams {
    fn default() -> Self {
        Self {
            natural_frequency: 2.5,
            reference_length: 0.24,
            frequency_range: (0.9, 6.0),

            damping_ratio: 0.62,
            density_damping: 0.14,
            compaction_damping: 0.22,
            damping_range: (0.45, 1.05),

            max_angle: 84.0 * std::f32::consts::PI / 180.0,
            high_angle_stiffness: 90.0,

            permanent_fraction: 0.45,
            fast_fraction: 0.35,
            slow_fraction: 0.20,

            correlation_cells: 0.8,
            coupling_falloff: 0.45,

            contact_frequency: (6.0, 10.0),
            contact_damping: 1.0,

            wind_frequency: 2.0,
            wind_damping: 0.9,

            fast_set: 0.15,
            fast_recover: 1.4,
            slow_set: 0.8,
            slow_recover: 30.0,
            compaction_set: 0.6,
            compaction_recover: 50.0,
            axis_set: 0.7,
            axis_recover: 30.0,

            dose_decay: 45.0,
            // Dose is in severity-seconds, so these read directly: a fraction
            // of a second of contact starts a mark, and a couple of seconds of
            // standing crushes the grass properly.
            dose_to_compaction: 1.8,
            fast_activation: (0.02, 0.35),
            slow_activation: (0.04, 0.50),
        }
    }
}

impl GrassParams {
    /// Sum of the three stiffness shares. Should be one.
    pub fn stiffness_share(&self) -> f32 {
        self.permanent_fraction + self.fast_fraction + self.slow_fraction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stiffness_shares_add_up() {
        // If these drifted apart the grass would be quietly stiffer or floppier
        // than its stated natural frequency, and every other tuning value would
        // be compensating for it.
        let share = GrassParams::default().stiffness_share();
        assert!((share - 1.0).abs() < 1e-5, "{share}");
    }

    #[test]
    fn some_stiffness_is_never_delegated_to_memory() {
        // Otherwise fully set memories leave grass flat forever.
        assert!(GrassParams::default().permanent_fraction > 0.1);
    }

    #[test]
    fn memory_branches_are_ordered_slow_after_fast() {
        let p = GrassParams::default();
        assert!(p.fast_set < p.slow_set);
        assert!(p.fast_recover < p.slow_recover);
        // And a trail must outlast a footstep by a wide margin, or there is no
        // point having two branches at all.
        assert!(p.slow_recover > p.fast_recover * 10.0);
    }

    #[test]
    fn contact_outranks_wind() {
        let p = GrassParams::default();
        assert!(p.contact_frequency.0 > p.wind_frequency);
        assert!(p.contact_frequency.0 < p.contact_frequency.1);
    }

    #[test]
    fn damping_is_high_enough_to_avoid_ringing() {
        // Mid-length grass in a canopy should not oscillate like a reed.
        let p = GrassParams::default();
        assert!(p.damping_ratio > 0.5, "{}", p.damping_ratio);
        assert!(p.damping_range.0 <= p.damping_ratio);
        assert!(p.damping_range.1 >= p.damping_ratio);
    }

    #[test]
    fn coupling_stays_weak() {
        // The single most likely way to make this look like rubber.
        assert!(GrassParams::default().correlation_cells <= 1.5);
    }

    #[test]
    fn the_angular_cap_is_short_of_flat() {
        let p = GrassParams::default();
        assert!(p.max_angle < std::f32::consts::FRAC_PI_2);
        assert!(p.max_angle > 70.0_f32.to_radians());
    }
}
