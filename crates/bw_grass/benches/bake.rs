//! What the grass costs.
//!
//! ```sh
//! cargo bench -p bw_grass
//! cargo bench -p bw_grass -- page_bake        # just the headline
//! cargo bench -p bw_grass -- --save-baseline before
//! cargo bench -p bw_grass -- --baseline before
//! ```
//!
//! The last two lines are the whole workflow. Save a baseline, optimise,
//! compare — criterion prints the change with a confidence interval, which is
//! the only honest way to read a five percent difference on a laptop.
//!
//! ## The number that matters is latency, not throughput
//!
//! Pages are baked on a background thread as the camera approaches them, one
//! page per task. So the question the renderer asks is never "how many pages a
//! second can this machine bake" — it is **"will this one page be finished
//! before the camera gets there"**, and that is a single-threaded latency on one
//! page of the size that actually ships.
//!
//! Everything here is therefore single-threaded, and [`bw_grass::bake::bake`] is
//! measured at [`bw_grass::bake::TILE_PIXELS`] rather than at some convenient
//! round number. Dividing a parallel sweep's wall clock by the number of pages
//! would measure throughput on a fully loaded machine and print it in the place
//! where latency belongs — a mistake worth naming, because the two numbers
//! differ here by the core count and only one of them decides whether the grass
//! pops in.
//!
//! ## What each group is for
//!
//! | Group | Question |
//! |---|---|
//! | `page_bake` | The shipping number: one page, one thread, three places |
//! | `page_stage` | **Which part of that page.** Five stages, timed apart |
//! | `stroke` | One mark, so the stroke pass divides into a count and a unit cost |
//! | `page_size` | Is cost proportional to area, or does the guard band dominate? |
//! | `seed_spread` | Does the world you are in change what a page costs? |
//! | `field_sample` | The composition fields, which every lattice point pays for |
//! | `blur` | The shading terms, which scale with radius and not with content |
//! | `resample` | Minification — what a mip chain would have to beat |
//!
//! `page_stage` is the one to read first. Everything else here refines a number
//! it has already localised, and its rows sum to `page_bake` — so a stage that
//! does not appear in it is time nobody is accounting for.
//!
//! `page_size` earns its place by answering a question an optimiser will
//! otherwise guess at. A page pays for a guard band around its edge, so its cost
//! has an area term and a perimeter term; if the perimeter term is large then
//! *fewer, larger* pages is a real optimisation and the draw-call problem and
//! the bake-cost problem have the same fix. If it is small, page size is free to
//! be chosen on streaming grounds alone.

use std::hint::black_box;

use bevy::prelude::*;
use bw_bench::SEEDS;
use bw_grass::bake::{
    BakeParams, Macro, Page, TILE_PIXELS, bake, lay_floor, plant_strokes, resolve,
};
use bw_grass::field::WorldField;
use bw_grass::fixtures::{PLACES, place_name};
use bw_grass::iso;
use bw_grass::stroke::{Painter, Stroke};
use bw_grass::surface::{Surface, blur, resample};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

/// A page bake is around a tenth of a second, so criterion's default hundred
/// samples would be ten seconds of measurement plus warm-up for every single
/// benchmark id. Twelve is enough to see through the noise on something this
/// long, and keeps the suite inside a couple of minutes.
const SAMPLES: usize = 12;

fn params(seed: u64) -> BakeParams {
    BakeParams { seed, ..default() }
}

/// One page, one thread, at the size the renderer streams.
fn page_bake(c: &mut Criterion) {
    let mut group = c.benchmark_group("page_bake");
    group.sample_size(SAMPLES);
    group.throughput(Throughput::Elements((TILE_PIXELS * TILE_PIXELS) as u64));

    let params = params(SEEDS[0]);
    for (index, origin) in PLACES.iter().enumerate() {
        group.bench_function(place_name(index), |b| {
            b.iter(|| {
                bake(
                    black_box(Page::new(*origin, TILE_PIXELS, TILE_PIXELS)),
                    black_box(&params),
                )
            })
        });
    }
    group.finish();
}

/// Does a page cost its area, or does its edge cost as much as its middle?
fn page_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("page_size");
    group.sample_size(SAMPLES);

    let params = params(SEEDS[0]);
    for side in [64usize, 128, 256, 512] {
        // Per-pixel, so the rows are directly comparable and a rising number
        // means the small pages are paying for their perimeter.
        group.throughput(Throughput::Elements((side * side) as u64));
        group.bench_with_input(BenchmarkId::from_parameter(side), &side, |b, &side| {
            b.iter(|| {
                bake(
                    black_box(Page::new(PLACES[0], side, side)),
                    black_box(&params),
                )
            })
        });
    }
    group.finish();
}

/// Whether the world you are in changes what a page costs.
///
/// Half a page, to keep ten seeds affordable. It is a spread measurement rather
/// than a level one — the level comes from `page_bake` — and a spread is visible
/// at any size.
fn seed_spread(c: &mut Criterion) {
    const SIDE: usize = 128;
    let mut group = c.benchmark_group("seed_spread");
    group.sample_size(SAMPLES);
    group.throughput(Throughput::Elements((SIDE * SIDE) as u64));

    for seed in SEEDS {
        let params = params(seed);
        group.bench_function(format!("{seed:#010x}"), |b| {
            b.iter(|| {
                bake(
                    black_box(Page::new(PLACES[0], SIDE, SIDE)),
                    black_box(&params),
                )
            })
        });
    }
    group.finish();
}

/// The composition fields, which every lattice point of every page pays for.
///
/// Sampled on the same lattice spacing the baker uses, over a page's worth of
/// ground, so the number is directly comparable to a slice of `page_bake` rather
/// than being an abstract per-call cost.
fn field_sample(c: &mut Criterion) {
    let mut group = c.benchmark_group("field_sample");
    let field = WorldField::new(SEEDS[0]);

    // One lattice's worth: the baker samples every sixth final pixel.
    let points: Vec<Vec2> = (0..(TILE_PIXELS / 6) * (TILE_PIXELS / 6))
        .map(|i| {
            let (x, y) = (i % (TILE_PIXELS / 6), i / (TILE_PIXELS / 6));
            iso::from_cache_ground(PLACES[0] + Vec2::new(x as f32, y as f32) * 6.0)
        })
        .collect();

    group.throughput(Throughput::Elements(points.len() as u64));
    group.bench_function("page_lattice", |b| {
        b.iter(|| {
            let mut total = 0.0f32;
            for point in black_box(&points) {
                total += field.sample(*point).height;
            }
            total
        })
    });
    group.finish();
}

/// The separable blur behind every shading term.
///
/// Its cost is set by the radius and the buffer, and not at all by what is in
/// them — which is why one buffer of arbitrary content is a fair measurement
/// here and would not be anywhere else in this file.
fn blur_radius(c: &mut Criterion) {
    let mut group = c.benchmark_group("blur");
    let source: Vec<f32> = (0..TILE_PIXELS * TILE_PIXELS)
        .map(|i| ((i * 37) % 101) as f32 / 100.0)
        .collect();

    group.throughput(Throughput::Elements(source.len() as u64));
    for radius in [2usize, 8, 32] {
        group.bench_with_input(
            BenchmarkId::from_parameter(radius),
            &radius,
            |b, &radius| {
                b.iter(|| {
                    blur(
                        black_box(&source),
                        TILE_PIXELS,
                        TILE_PIXELS,
                        black_box(radius),
                    )
                })
            },
        );
    }
    group.finish();
}

/// Minification: the cost of showing a baked page at the size it is seen.
///
/// Not on the frame path today — the GPU samples the page directly — but it is
/// the whole cost of the snapshot suite, and it is the shape of the work a mip
/// chain would do. Sized at the ratio the shipping camera height produces.
fn resample_view(c: &mut Criterion) {
    const SIDE: usize = 1024;
    let mut group = c.benchmark_group("resample");
    let source: Vec<Vec3> = (0..SIDE * SIDE)
        .map(|i| Vec3::splat(((i * 41) % 97) as f32 / 96.0))
        .collect();

    let (_, _, scale) = iso::view_pixels(bw_grass::fixtures::BATTLE_VIEW, (1920, 1080));
    let target = (SIDE as f32 * scale) as usize;

    group.throughput(Throughput::Elements((SIDE * SIDE) as u64));
    group.bench_function("battle_view", |b| {
        b.iter(|| {
            resample(
                black_box(&source),
                SIDE,
                SIDE,
                black_box(target),
                black_box(target),
            )
        })
    });
    group.finish();
}

/// Where a page's time actually goes.
///
/// The group that decides what to optimise, and the one nothing else in this
/// file can substitute for. `page_bake` says a page costs what it costs;
/// `page_stage` says which fifth of it to attack, and the answer is not
/// guessable from the source. Two of these rows read the opposite of the truth:
///
/// - **`fields`** looks like per-page setup and is not work at all. A
///   [`WorldField`] is a seed and a light vector; every field it names is
///   evaluated inside `sample`. The row is kept because "constructing the world
///   is free, and all of it is per-sample" is a fact worth having a number
///   beside — and because the day someone precomputes a table in there, this is
///   the row that will say so.
/// - **`shade`** looks like a resolve step and is shaped like a second
///   rasteriser: it runs over the supersampled buffer — nine times the final
///   pixel count — plus several blurs.
///
/// The stages are timed in isolation but built on real inputs: each one is
/// handed the exact state the one before it would have produced, so nothing here
/// is measuring a cold cache the shipping path never sees.
fn page_stage(c: &mut Criterion) {
    let mut group = c.benchmark_group("page_stage");
    group.sample_size(SAMPLES);
    group.throughput(Throughput::Elements((TILE_PIXELS * TILE_PIXELS) as u64));

    let params = params(SEEDS[0]);
    let page = Page::new(PLACES[0], TILE_PIXELS, TILE_PIXELS);

    group.bench_function("fields", |b| {
        b.iter(|| {
            black_box(WorldField::lit_by(
                black_box(params.seed),
                black_box(params.light),
            ))
        })
    });

    let field = WorldField::lit_by(params.seed, params.light);
    group.bench_function("lattice", |b| {
        b.iter(|| Macro::build(black_box(&page), black_box(&field)))
    });

    // Allocation is its own row because it is not small: six channels over a
    // 3x supersampled page is more than five million entries to zero, and an
    // optimisation that reuses the buffer between pages would collect exactly
    // this much.
    group.bench_function("allocate", |b| {
        b.iter(|| Surface::new(black_box(TILE_PIXELS), black_box(TILE_PIXELS)))
    });

    let lattice = Macro::build(&page, &field);
    group.bench_function("floor", |b| {
        b.iter_batched_ref(
            || Surface::new(TILE_PIXELS, TILE_PIXELS),
            |surface| lay_floor(surface, black_box(&page), &field, &lattice),
            BatchSize::PerIteration,
        )
    });

    // The stroke pass wants a floored surface, not a blank one — the depth test
    // it runs against is the floor's.
    let floored = || {
        let mut surface = Surface::new(TILE_PIXELS, TILE_PIXELS);
        lay_floor(&mut surface, &page, &field, &lattice);
        surface
    };
    group.bench_function("strokes", |b| {
        b.iter_batched_ref(
            floored,
            |surface| plant_strokes(surface, black_box(&page), &field, &params),
            BatchSize::PerIteration,
        )
    });

    let mut planted = floored();
    plant_strokes(&mut planted, &page, &field, &params);
    group.bench_function("shade", |b| {
        b.iter(|| resolve(black_box(&planted), &page, &lattice, &params))
    });

    // A slice out of `shade`, so that stage is not a single opaque number
    // either. It is the cheap half — one box filter over the supersampled
    // height and occupancy channels — and knowing its size is what says the
    // rest of `shade` is the per-supersampled-pixel ramp work, which is where
    // an optimisation would have to go.
    group.bench_function("shade.height_maps", |b| {
        b.iter(|| planted.height_maps(black_box(TILE_PIXELS), black_box(TILE_PIXELS)))
    });

    group.finish();
}

/// One mark, drawn.
///
/// The innermost loop of the whole system, and the unit every stroke-count
/// decision is denominated in. A page holds thousands of these, so `page_stage`
/// divided by this number is the honest answer to "is the stroke pass slow
/// because each mark is expensive, or because there are a great many of them" —
/// and those two findings have nothing in common as repairs.
///
/// The surface is reused across iterations on purpose. Reallocating it would put
/// five million entries of `memset` inside a measurement of one blade.
fn stroke_draw(c: &mut Criterion) {
    let mut group = c.benchmark_group("stroke");
    let params = params(SEEDS[0]);
    let mut surface = Surface::new(TILE_PIXELS, TILE_PIXELS);
    let mut painter = Painter::new(&mut surface, PLACES[0], params.light);

    for (name, stroke) in [
        ("blade", Stroke::default()),
        (
            "long_blade",
            Stroke {
                length: 0.34,
                ..Stroke::default()
            },
        ),
    ] {
        group.bench_function(name, |b| b.iter(|| painter.draw(black_box(&stroke))));
    }
    group.finish();
}

criterion_group!(
    benches,
    page_bake,
    page_stage,
    stroke_draw,
    page_size,
    seed_spread,
    field_sample,
    blur_radius,
    resample_view
);
criterion_main!(benches);
