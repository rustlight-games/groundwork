//! Do two independently baked pages agree along their join?
//!
//! The characteristic failure of a terrain framework, and the one hardest to
//! notice from a single plate: two pieces of ground computed separately that
//! disagree by a little along the line where they meet. On a static image it
//! reads as a faint grid; the moment the camera pans, it reads as *wrong* and
//! nobody can say why.
//!
//! ## Split equivalence is the whole test
//!
//! Bake a region as one plate. Bake the same region as four and stitch them.
//! Subtract. Anything but zero is a term that depended on where the rectangle's
//! edges happened to be — which is precisely the class of bug that makes a
//! generator irreproducible at a different tiling.
//!
//! The measurement is deliberately *not* forgiving. A seam of one code value is
//! visible on flat ground: the eye is far better at finding a straight edge in
//! noise than at finding a shape, and a one-value step that runs for two
//! thousand texels in a straight line is exactly what it is best at.
//!
//! ## Three different tolerances, for three different reasons
//!
//! - **Material weights** must be bit-identical. They come from a pure function
//!   of world position, so anything else means the function is not pure.
//! - **Elevation** is quantised to a tenth of a millimetre, because it falls out
//!   of transcendental functions whose last bit is arithmetic noise.
//! - **Colour** is compared perceptually, because a renderer's output legitimately
//!   depends on filtering and sample counts, and demanding equality there would
//!   fail on a change that improved the picture.

/// How two plates of the same ground differ.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SeamError {
    /// The largest single difference found.
    pub worst: f64,
    /// The mean over every compared texel.
    pub mean: f64,
    /// How many texels differed at all.
    pub differing: usize,
    pub compared: usize,
}

impl SeamError {
    /// Whether nothing differed.
    pub fn is_exact(self) -> bool {
        self.differing == 0
    }

    /// What fraction of the compared texels differed.
    pub fn fraction(self) -> f64 {
        if self.compared == 0 {
            return 0.0;
        }
        self.differing as f64 / self.compared as f64
    }
}

/// Compare two plates of the same ground, texel for texel.
///
/// Returns `None` for mismatched sizes rather than comparing what it can: two
/// plates of different sizes are not two bakes of the same ground, and reporting
/// a seam error for them would be answering a question nobody asked.
pub fn compare(a: &[f32], b: &[f32], tolerance: f64) -> Option<SeamError> {
    if a.len() != b.len() {
        return None;
    }
    let mut error = SeamError {
        compared: a.len(),
        ..SeamError::default()
    };
    let mut total = 0.0;
    for (x, y) in a.iter().zip(b) {
        let difference = (*x as f64 - *y as f64).abs();
        total += difference;
        if difference > tolerance {
            error.differing += 1;
            error.worst = error.worst.max(difference);
        }
    }
    error.mean = total / a.len().max(1) as f64;
    Some(error)
}

/// Cut one page out of a larger plate.
///
/// The operation split equivalence rests on, and it is worth having in one
/// place: an off-by-one in a crop reports a seam error that is really a
/// measurement bug, and then somebody spends a day looking for it in the
/// generator.
pub fn crop(
    plate: &[f32],
    plate_width: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> Option<Vec<f32>> {
    let plate_height = plate.len() / plate_width.max(1);
    if x + width > plate_width || y + height > plate_height {
        return None;
    }
    let mut out = Vec::with_capacity(width * height);
    for row in 0..height {
        let start = (y + row) * plate_width + x;
        out.extend_from_slice(&plate[start..start + width]);
    }
    Some(out)
}

/// Stitch a grid of pages back into one plate.
///
/// The inverse of [`crop`], and having both makes the round trip testable
/// without a generator anywhere near it.
pub fn stitch(
    pages: &[Vec<f32>],
    pages_across: usize,
    page_width: usize,
    page_height: usize,
) -> Option<Vec<f32>> {
    if pages.is_empty() || pages_across == 0 || !pages.len().is_multiple_of(pages_across) {
        return None;
    }
    if pages
        .iter()
        .any(|page| page.len() != page_width * page_height)
    {
        return None;
    }
    let pages_down = pages.len() / pages_across;
    let width = pages_across * page_width;
    let mut plate = vec![0.0f32; width * pages_down * page_height];
    for (index, page) in pages.iter().enumerate() {
        let (px, py) = (index % pages_across, index / pages_across);
        for row in 0..page_height {
            let target = (py * page_height + row) * width + px * page_width;
            plate[target..target + page_width]
                .copy_from_slice(&page[row * page_width..(row + 1) * page_width]);
        }
    }
    Some(plate)
}

/// The tolerance a channel is compared at.
///
/// See the module note: three different numbers for three different reasons,
/// written down here so a caller does not invent a fourth.
pub mod tolerance {
    /// Material weights come from a pure function of world position. Anything
    /// but zero means the function is not pure.
    pub const MATERIAL_WEIGHT: f64 = 0.0;

    /// A tenth of a millimetre. Elevation falls out of transcendental functions
    /// whose last bit is arithmetic noise, and relief runs to a quarter of a
    /// metre — so this is four parts in ten thousand and a real change cannot
    /// hide under it.
    pub const ELEVATION_M: f64 = 1.0e-4;

    /// One code value in eight bits. A seam of one value is visible on flat
    /// ground, because the eye is far better at finding a straight edge in noise
    /// than at finding a shape.
    pub const COLOUR: f64 = 1.0 / 255.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(width: usize, height: usize) -> Vec<f32> {
        (0..width * height).map(|i| i as f32).collect()
    }

    #[test]
    fn a_plate_split_and_stitched_is_the_plate_it_came_from() {
        // The round trip, without a generator anywhere near it. An off-by-one
        // here would report a seam error that is really a measurement bug.
        let plate = ramp(8, 8);
        let pages: Vec<Vec<f32>> = (0..4)
            .map(|i| crop(&plate, 8, (i % 2) * 4, (i / 2) * 4, 4, 4).expect("in bounds"))
            .collect();
        let stitched = stitch(&pages, 2, 4, 4).expect("well formed");
        assert_eq!(stitched, plate);
        assert!(
            compare(&stitched, &plate, 0.0)
                .expect("same size")
                .is_exact()
        );
    }

    #[test]
    fn a_crop_takes_the_texels_it_says() {
        let plate = ramp(4, 4);
        let corner = crop(&plate, 4, 1, 1, 2, 2).expect("in bounds");
        assert_eq!(corner, vec![5.0, 6.0, 9.0, 10.0]);
    }

    #[test]
    fn a_crop_past_the_edge_is_refused_rather_than_clamped() {
        // Clamping would return a smaller page than asked for, and the caller
        // would then compare it against a full one and report a size mismatch
        // several steps from the cause.
        let plate = ramp(4, 4);
        assert_eq!(crop(&plate, 4, 3, 0, 2, 2), None);
        assert_eq!(crop(&plate, 4, 0, 3, 2, 2), None);
        assert!(crop(&plate, 4, 2, 2, 2, 2).is_some());
    }

    #[test]
    fn a_malformed_stitch_is_refused() {
        assert_eq!(stitch(&[], 2, 4, 4), None);
        // Three pages into a two-wide grid.
        assert_eq!(
            stitch(&[vec![0.0; 16], vec![0.0; 16], vec![0.0; 16]], 2, 4, 4),
            None
        );
        // A page of the wrong size.
        assert_eq!(stitch(&[vec![0.0; 16], vec![0.0; 15]], 2, 4, 4), None);
    }

    #[test]
    fn a_single_texel_seam_is_reported() {
        // The failure this whole module exists for. One code value, on one
        // texel, and the measurement has to see it.
        let a = vec![0.5f32; 64];
        let mut b = a.clone();
        b[32] += 1.0 / 255.0 + 1.0e-6;
        let error = compare(&a, &b, tolerance::MATERIAL_WEIGHT).expect("same size");
        assert!(!error.is_exact());
        assert_eq!(error.differing, 1);
        assert!(error.worst > 0.0);
        assert!((error.fraction() - 1.0 / 64.0).abs() < 1.0e-9);
    }

    #[test]
    fn material_weights_are_compared_exactly() {
        // They come from a pure function of world position, so anything but
        // zero means the function is not pure.
        assert_eq!(tolerance::MATERIAL_WEIGHT, 0.0);
        let a = vec![0.5f32; 16];
        let mut b = a.clone();
        b[0] = 0.5 + f32::EPSILON;
        let error = compare(&a, &b, tolerance::MATERIAL_WEIGHT).expect("same size");
        assert!(!error.is_exact(), "a one-ulp difference was tolerated");
    }

    #[test]
    fn elevation_tolerates_arithmetic_noise_and_not_a_real_change() {
        let a = vec![0.1f32; 16];
        let mut noise = a.clone();
        noise[0] = 0.1 + 1.0e-7;
        assert!(
            compare(&a, &noise, tolerance::ELEVATION_M)
                .expect("same size")
                .is_exact()
        );

        let mut real = a.clone();
        real[0] = 0.1 + 1.0e-3;
        assert!(
            !compare(&a, &real, tolerance::ELEVATION_M)
                .expect("same size")
                .is_exact()
        );
    }

    #[test]
    fn mismatched_sizes_are_refused_rather_than_partially_compared() {
        // Two plates of different sizes are not two bakes of the same ground,
        // and reporting a seam error for them answers a question nobody asked.
        assert_eq!(compare(&[0.0; 4], &[0.0; 8], 0.0), None);
    }

    #[test]
    fn an_identical_pair_reports_nothing() {
        let plate = ramp(8, 8);
        let error = compare(&plate, &plate, 0.0).expect("same size");
        assert!(error.is_exact());
        assert_eq!(error.mean, 0.0);
        assert_eq!(error.worst, 0.0);
        assert_eq!(error.fraction(), 0.0);
        assert_eq!(error.compared, 64);
    }

    #[test]
    fn the_mean_counts_every_texel_and_the_worst_only_the_failures() {
        // Two numbers because they answer different questions: a mean says how
        // wrong the plate is overall, and the worst says whether there is a
        // visible line in it. A seam is one texel wide and enormous; a filter
        // change is every texel and tiny.
        let a = vec![0.0f32; 100];
        let mut b = a.clone();
        b[0] = 1.0;
        let error = compare(&a, &b, 0.0).expect("same size");
        assert_eq!(error.worst, 1.0);
        assert!((error.mean - 0.01).abs() < 1.0e-9);
        assert_eq!(error.differing, 1);
    }
}
