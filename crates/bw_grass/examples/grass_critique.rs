//! Measure a plate against reference art, in absolute numbers.
//!
//! ```sh
//! cargo run --release -p bw_grass --example grass_critique -- ours.png
//! cargo run --release -p bw_grass --example grass_critique -- ours.png --target ref.png
//! cargo run --release -p bw_grass --example grass_critique -- ours.png --crop 1024
//! ```
//!
//! This is the **look gate**, and it is the counterpart to `grass_snapshot`
//! rather than a replacement for it. The snapshot answers "did the picture
//! move"; a deliberate look change moves it entirely and the answer stops
//! meaning anything. This answers "is the picture the one we are aiming at",
//! which stays meaningful across a rewrite because it needs no pixel
//! correspondence between the two images at all.
//!
//! See [`bw_grass::critique`] for what each number fails on. The exit status is
//! non-zero when a gated band is missed, so it can sit in a check script.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bw_grass::critique::{BANDS, Critique};

fn main() {
    let options = Options::parse();
    let Some(ours) = load(&options.plate, options.crop) else {
        std::process::exit(2);
    };
    let measured = Critique::of(&ours.0, ours.1, ours.2);

    let target = options.target.as_ref().and_then(|path| {
        let loaded = load(path, options.crop)?;
        Some(Critique::of(&loaded.0, loaded.1, loaded.2))
    });

    println!(
        "{} — {}x{}",
        options.plate.display(),
        measured.width,
        measured.height
    );
    if let Some(path) = &options.target {
        println!("against {}", path.display());
    }
    println!();
    print!("{}", measured.table(target.as_ref()));
    println!();

    let failures = measured.failures();
    if failures.is_empty() {
        println!("all {} gated bands hold", BANDS.len());
    } else {
        for failure in &failures {
            println!("OUT OF BAND  {failure}");
        }
        std::process::exit(1);
    }
}

/// Read a PNG as display-referred RGB, optionally taking a centred square.
///
/// The crop matters more than it looks. Reference art usually has something in
/// it that is not the subject — a tree, a rock, a corner of wood — and a median
/// luminance measured across that is a median of two different things.
fn load(path: &Path, crop: Option<usize>) -> Option<(Vec<Vec3>, usize, usize)> {
    let image = match image::open(path) {
        Ok(image) => image.to_rgb8(),
        Err(error) => {
            eprintln!("cannot read {}: {error}", path.display());
            return None;
        }
    };
    let (width, height) = (image.width() as usize, image.height() as usize);
    let (x0, y0, w, h) = match crop {
        Some(side) if side < width.min(height) => {
            ((width - side) / 2, (height - side) / 2, side, side)
        }
        _ => (0, 0, width, height),
    };

    let mut pixels = Vec::with_capacity(w * h);
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let p = image.get_pixel(x as u32, y as u32);
            pixels.push(Vec3::new(
                p[0] as f32 / 255.0,
                p[1] as f32 / 255.0,
                p[2] as f32 / 255.0,
            ));
        }
    }
    Some((pixels, w, h))
}

struct Options {
    plate: PathBuf,
    target: Option<PathBuf>,
    crop: Option<usize>,
}

impl Options {
    fn parse() -> Self {
        let mut options = Self {
            plate: PathBuf::from("target/grass_plate.png"),
            target: None,
            crop: None,
        };
        let mut positional = false;
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let mut value = || arguments.next().unwrap_or_default();
            match argument.as_str() {
                "--target" => options.target = Some(PathBuf::from(value())),
                "--crop" => options.crop = value().parse().ok(),
                "--help" | "-h" => {
                    println!("grass_critique <plate.png> [--target art.png] [--crop PX]");
                    std::process::exit(0);
                }
                other if !other.starts_with("--") && !positional => {
                    options.plate = PathBuf::from(other);
                    positional = true;
                }
                other => eprintln!("ignoring unknown argument {other}"),
            }
        }
        options
    }
}
