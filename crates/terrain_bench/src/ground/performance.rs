//! What a run cost, and on what.
//!
//! ## Why a speed number without a quality number is worthless
//!
//! The easiest way to make any of this faster is to do less of it: fewer
//! samples, fewer bands, a coarser lattice. Every one of those produces a real
//! speed-up and a worse surface, and a timing report on its own cannot tell the
//! difference between an optimisation and a quality-tier change.
//!
//! So every timing here is paired with the counters from the same run — analysis
//! samples, spectral samples, relief bands — and the comparison rule is stated
//! rather than implied: **no speed claim is valid unless the compared runs have
//! equal content counts and their morphology metrics are inside the gate.**
//!
//! ## Why median and MAD rather than mean and standard deviation
//!
//! A benchmark on a developer machine competes with a browser, an indexer and a
//! compiler. Those produce outliers in one direction only, and a mean absorbs
//! them while a median does not. The MAD is reported beside it because the two
//! disagreeing is itself the signal: a stage whose median is stable and whose
//! MAD is large is a stage something else on the machine is interfering with,
//! and its number should not be believed.
//!
//! ## What is deliberately not recorded
//!
//! No wall-clock timestamp, and no thread count. A report that changed every
//! time it was regenerated could not be committed, and a committed report is the
//! whole point — the question this exists to answer is *why did the ground move*,
//! asked a month later.

use std::collections::BTreeMap;
use std::time::Instant;

/// What one stage of a run cost.
#[derive(Clone, Debug, PartialEq)]
pub struct StageTiming {
    pub stage: String,
    pub median_ms: f64,
    /// Median absolute deviation. See the module note.
    pub mad_ms: f64,
    pub p95_ms: f64,
    pub repetitions: usize,
    /// The bytes this stage's own working set occupies.
    ///
    /// **Declared by the caller, not observed by an allocator.** There is no
    /// allocator hook in this workspace, and a number produced by guessing at
    /// one would be worse than an honest zero: a reader comparing two runs would
    /// believe it. What a caller can say truthfully is how large the buffers it
    /// allocated are, and that is what this is — so a stage that never says
    /// reports nothing rather than something made up.
    pub peak_bytes: usize,
}

/// The machine a run happened on.
///
/// Recorded because a timing compared across machines is not a comparison. Only
/// the facts that are stable for a given checkout and host: no timestamp,
/// nothing that would make a committed report churn.
///
/// ## The renderer fields say there is no renderer
///
/// The report schema requires `gpu`, `blender` and `cycles_device`, because it
/// was written for a contract that spans both halves of this pipeline. This
/// benchmark is the geometry half and has no renderer in it at all — that is the
/// point of it running in a second — so those fields carry a sentence saying so
/// rather than a plausible-looking value.
///
/// The distinction matters to a reader comparing two reports. "None, this
/// benchmark has no renderer" and "an RTX 4090" are different facts; "unknown"
/// and an empty string are the same fact badly told, and a reader who sees one
/// of those has to go and find out which.
#[derive(Clone, Debug, PartialEq)]
pub struct MachineIdentity {
    pub os: String,
    pub arch: String,
    pub cpu: String,
    pub gpu: String,
    /// The compiler that built this binary, from the build script.
    pub rustc: String,
    /// `dev`, `release`, or whatever the profile was.
    pub cargo_profile: String,
    pub blender: String,
    pub cycles_device: String,
    pub logical_threads: usize,
}

/// What the renderer fields carry when there is no renderer.
const NO_RENDERER: &str = "none (this benchmark has no renderer in the loop)";

impl MachineIdentity {
    pub fn detect() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            // The architecture and the thread count, which is every fact about
            // the processor this crate can learn without shelling out. A
            // benchmark that ran `sysctl` to name a chip would be a benchmark
            // with a platform-specific side effect in it, and the model name
            // buys a reader nothing the thread count does not.
            cpu: format!("{} ({} logical)", std::env::consts::ARCH, logical_threads()),
            gpu: NO_RENDERER.to_string(),
            // From the build script, so it is the compiler that built *this*
            // binary rather than whatever is first on the path when it runs.
            rustc: option_env!("TERRAIN_BENCH_RUSTC")
                .unwrap_or("unknown (no build script ran)")
                .to_string(),
            cargo_profile: if cfg!(debug_assertions) {
                "dev".to_string()
            } else {
                "release".to_string()
            },
            blender: NO_RENDERER.to_string(),
            cycles_device: NO_RENDERER.to_string(),
            logical_threads: logical_threads(),
        }
    }
}

fn logical_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Every timing from one run, and what it was measuring.
#[derive(Clone, Debug, PartialEq)]
pub struct PerformanceMetrics {
    pub machine: MachineIdentity,
    pub repetitions: usize,
    pub warmup_repetitions: usize,
    pub total_median_ms: f64,
    pub total_p95_ms: f64,
    /// The largest working set any single stage declared. See [`StageTiming`].
    pub peak_bytes: usize,
    pub stages: Vec<StageTiming>,
    /// The counters the timings must be read against.
    ///
    /// Carried *in the same structure* rather than left for a reader to
    /// correlate. A stage that got faster by measuring fewer samples is a
    /// quality change, and the only way to see it is to have both numbers in
    /// front of you.
    pub counters: BTreeMap<String, usize>,
}

/// Collects stage timings across repetitions.
///
/// ## Why the warm-up is separate rather than discarded
///
/// The first repetition of anything on a modern machine pays for cold caches,
/// lazy statics and a page-faulted heap, and folding it into the sample makes
/// every median wrong by an amount that depends on how many repetitions ran.
/// Running it and *naming* it is honest; running it and silently dropping it
/// makes the repetition count mean something different from what it says.
#[derive(Debug, Default)]
pub struct Recorder {
    samples: BTreeMap<String, Vec<f64>>,
    order: Vec<String>,
    counters: BTreeMap<String, usize>,
    bytes: BTreeMap<String, usize>,
    repetitions: usize,
    warmups: usize,
}

impl Recorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Note a counter this run's timings must be read against.
    pub fn count(&mut self, key: &str, value: usize) {
        self.counters.insert(key.to_string(), value);
    }

    /// Declare how large one stage's working set is.
    ///
    /// The largest value ever declared for a stage wins, because a stage that
    /// allocated more on one repetition allocated that much.
    pub fn bytes(&mut self, stage: &str, value: usize) {
        let slot = self.bytes.entry(stage.to_string()).or_default();
        *slot = (*slot).max(value);
    }

    /// Time one stage of one repetition.
    ///
    /// Returns whatever the stage returned, so a caller threads its real work
    /// through rather than running it twice — once for the answer and once for
    /// the clock, which is a mistake that reports half the true cost.
    pub fn stage<T>(&mut self, name: &str, work: impl FnOnce() -> T) -> T {
        // Progress and measurement, which is the sanctioned use — nothing here
        // reaches a generator or a digest.
        #[allow(clippy::disallowed_types)]
        let started = Instant::now();
        let out = work();
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if !self.samples.contains_key(name) {
            self.order.push(name.to_string());
        }
        self.samples
            .entry(name.to_string())
            .or_default()
            .push(elapsed);
        out
    }

    /// Throw away everything measured so far, keeping the counters.
    ///
    /// What a warm-up is: the work happened and its timings are not evidence.
    pub fn discard_as_warmup(&mut self) {
        self.samples.clear();
        // The order too, and forgetting it was a real bug: `stage` appends a
        // name whenever `samples` does not already hold it, so a cleared
        // `samples` and a kept `order` meant every stage was listed twice and
        // the total was double what the run cost.
        self.order.clear();
        self.warmups += 1;
    }

    pub fn finish_repetition(&mut self) {
        self.repetitions += 1;
    }

    /// The report.
    pub fn finish(self) -> PerformanceMetrics {
        let stages: Vec<StageTiming> = self
            .order
            .iter()
            .filter_map(|name| {
                let samples = self.samples.get(name)?;
                if samples.is_empty() {
                    return None;
                }
                let mut sorted = samples.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let median = percentile(&sorted, 0.5);
                let mut deviations: Vec<f64> = sorted.iter().map(|v| (v - median).abs()).collect();
                deviations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                Some(StageTiming {
                    stage: name.clone(),
                    median_ms: median,
                    mad_ms: percentile(&deviations, 0.5),
                    p95_ms: percentile(&sorted, 0.95),
                    repetitions: sorted.len(),
                    peak_bytes: self.bytes.get(name).copied().unwrap_or(0),
                })
            })
            .collect();

        // The total is the sum of the stage medians rather than the median of
        // the totals. They differ, and this one is the useful decomposition:
        // it adds up to the parts a reader is being shown, so a stage that grew
        // is visible as the reason the total grew.
        let total_median: f64 = stages.iter().map(|s| s.median_ms).sum();
        let total_p95: f64 = stages.iter().map(|s| s.p95_ms).sum();

        // The largest single stage rather than the sum. The stages run one
        // after another and their buffers are dropped between, so a sum would
        // describe a program that never existed.
        let peak_bytes = stages.iter().map(|s| s.peak_bytes).max().unwrap_or(0);

        PerformanceMetrics {
            machine: MachineIdentity::detect(),
            repetitions: self.repetitions.max(1),
            warmup_repetitions: self.warmups,
            total_median_ms: total_median,
            total_p95_ms: total_p95,
            peak_bytes,
            stages,
            counters: self.counters,
        }
    }
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let position = fraction.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    let weight = position - low as f64;
    sorted[low] * (1.0 - weight) + sorted[high] * weight
}

/// One file a run wrote, and what it contains.
#[derive(Clone, Debug, PartialEq)]
pub struct ArtifactRecord {
    pub kind: String,
    pub path: String,
    /// A digest of the bytes, so a stale artifact cannot be mistaken for a
    /// fresh one.
    pub checksum: String,
    pub bytes: usize,
}

/// Write a plane of floats and record it.
///
/// Little-endian `f32`, which is what every reader of these already expects.
/// The checksum is over the bytes rather than over the values, because what a
/// comparison tool needs to know is whether the *file* changed.
pub fn write_plane(
    directory: &std::path::Path,
    name: &str,
    values: &[f32],
) -> std::io::Result<ArtifactRecord> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let path = directory.join(format!("{name}.f32"));
    std::fs::write(&path, &bytes)?;
    Ok(ArtifactRecord {
        kind: "plane".to_string(),
        path: format!("{name}.f32"),
        checksum: checksum(&bytes),
        bytes: bytes.len(),
    })
}

/// A stable digest of some bytes, as lowercase hex.
///
/// FNV-1a over 64 bits. Not cryptographic and not trying to be: the threat is a
/// stale file being mistaken for a fresh one, not an adversary.
pub fn checksum(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut state = OFFSET;
    for byte in bytes {
        state ^= *byte as u64;
        state = state.wrapping_mul(PRIME);
    }
    format!("{state:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stage_returns_what_it_measured() {
        // So a caller threads its real work through rather than running it
        // twice — once for the answer and once for the clock, which reports
        // half the true cost.
        let mut recorder = Recorder::new();
        let answer = recorder.stage("work", || 6 * 7);
        assert_eq!(answer, 42);
    }

    #[test]
    fn stages_are_reported_in_the_order_they_first_ran() {
        // Not alphabetically. A performance report reads as a pipeline, and a
        // reader looking for where the time went follows it in order.
        let mut recorder = Recorder::new();
        for _ in 0..3 {
            recorder.stage("sample", || {});
            recorder.stage("analyse", || {});
            recorder.stage("compare", || {});
            recorder.finish_repetition();
        }
        let metrics = recorder.finish();
        let names: Vec<&str> = metrics.stages.iter().map(|s| s.stage.as_str()).collect();
        assert_eq!(names, vec!["sample", "analyse", "compare"]);
        assert_eq!(metrics.repetitions, 3);
        for stage in &metrics.stages {
            assert_eq!(stage.repetitions, 3);
        }
    }

    #[test]
    fn a_warmup_is_discarded_and_counted_rather_than_hidden() {
        // Running it and naming it is honest; running it and silently dropping
        // it makes the repetition count mean something different from what it
        // says.
        let mut recorder = Recorder::new();
        recorder.stage("work", || {});
        recorder.discard_as_warmup();
        for _ in 0..4 {
            recorder.stage("work", || {});
            recorder.finish_repetition();
        }
        let metrics = recorder.finish();
        assert_eq!(metrics.warmup_repetitions, 1);
        assert_eq!(metrics.stages[0].repetitions, 4);
    }

    #[test]
    fn a_declared_working_set_is_the_largest_a_stage_ever_claimed() {
        // Because a stage that allocated more on one repetition allocated that
        // much, and the report's own peak is the largest single stage rather
        // than a sum over stages that never coexisted.
        let mut recorder = Recorder::new();
        recorder.bytes("wide", 4096);
        recorder.bytes("wide", 1024);
        recorder.bytes("narrow", 64);
        recorder.stage("wide", || {});
        recorder.stage("narrow", || {});
        recorder.finish_repetition();
        let metrics = recorder.finish();
        assert_eq!(metrics.stages[0].peak_bytes, 4096);
        assert_eq!(metrics.stages[1].peak_bytes, 64);
        assert_eq!(metrics.peak_bytes, 4096);
    }

    #[test]
    fn a_warmup_does_not_leave_a_second_copy_of_every_stage() {
        // The list of stage *names* has to be cleared with the samples. It was
        // not, once, and the report listed every stage twice with a total to
        // match — a doubled benchmark figure that no gate would have caught.
        let mut recorder = Recorder::new();
        recorder.stage("a", || {});
        recorder.stage("b", || {});
        recorder.discard_as_warmup();
        recorder.stage("a", || {});
        recorder.stage("b", || {});
        recorder.finish_repetition();
        let metrics = recorder.finish();
        let names: Vec<&str> = metrics.stages.iter().map(|s| s.stage.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn the_counters_travel_with_the_timings() {
        // A stage that got faster by measuring fewer samples is a quality
        // change, and the only way to see it is to have both numbers in front
        // of you.
        let mut recorder = Recorder::new();
        recorder.count("analysis_samples", 25921);
        recorder.stage("work", || {});
        recorder.finish_repetition();
        let metrics = recorder.finish();
        assert_eq!(metrics.counters.get("analysis_samples"), Some(&25921));
    }

    #[test]
    fn an_empty_run_reports_zero_rather_than_dividing_by_it() {
        let metrics = Recorder::new().finish();
        assert_eq!(metrics.total_median_ms, 0.0);
        assert!(metrics.stages.is_empty());
        // And still names a repetition, so a reader dividing by it is safe.
        assert_eq!(metrics.repetitions, 1);
    }

    #[test]
    fn the_total_is_the_sum_of_the_parts_a_reader_is_shown() {
        // Rather than the median of the totals, which is a different number and
        // does not decompose: a stage that grew has to be visible as the reason
        // the total grew.
        let mut recorder = Recorder::new();
        recorder.stage("a", || {});
        recorder.stage("b", || {});
        recorder.finish_repetition();
        let metrics = recorder.finish();
        let summed: f64 = metrics.stages.iter().map(|s| s.median_ms).sum();
        assert!((metrics.total_median_ms - summed).abs() < 1.0e-12);
    }

    #[test]
    fn a_checksum_moves_with_any_byte_and_is_stable_across_runs() {
        // The threat is a stale file mistaken for a fresh one, so what matters
        // is that a changed file changes and an unchanged one does not.
        assert_eq!(checksum(b"meadow"), checksum(b"meadow"));
        assert_ne!(checksum(b"meadow"), checksum(b"meadox"));
        assert_ne!(checksum(b"meadow"), checksum(b"meadow "));
        assert_eq!(checksum(b"").len(), 16);
    }

    #[test]
    fn a_written_plane_records_its_own_length_and_digest() {
        let directory = std::env::temp_dir().join("groundwork-perf-plane");
        std::fs::create_dir_all(&directory).expect("a scratch directory");
        let values: Vec<f32> = (0..64).map(|i| i as f32 * 0.25).collect();
        let record = write_plane(&directory, "height", &values).expect("writes");
        assert_eq!(record.bytes, values.len() * 4);
        assert_eq!(record.path, "height.f32");
        let written = std::fs::read(directory.join("height.f32")).expect("reads back");
        assert_eq!(written.len(), record.bytes);
        assert_eq!(checksum(&written), record.checksum);
    }

    #[test]
    fn the_machine_identity_holds_nothing_that_churns() {
        // A report that changed every time it was regenerated could not be
        // committed, and a committed report is the whole point.
        let a = MachineIdentity::detect();
        let b = MachineIdentity::detect();
        assert_eq!(a, b);
        assert!(!a.os.is_empty() && !a.arch.is_empty());
        // The schema requires every one of these to be a non-empty string, and
        // the honest value for the renderer fields is a sentence saying there
        // is no renderer rather than an empty string or a guess.
        for field in [&a.cpu, &a.gpu, &a.rustc, &a.blender, &a.cycles_device] {
            assert!(!field.is_empty());
        }
        assert!(a.logical_threads >= 1);
    }
}
