//! The cost of a Tesseract engine, and of the work it does.
//!
//! An engine is built once per document today. This benchmark separates that
//! construction from recognition on a real scan so the per-document overhead is
//! a measured number rather than an assumption:
//!
//! ```text
//! cargo bench -p pdfextractor-ocr-engine --bench engine_cost
//! ```
//!
//! `BENCH_ITERS` overrides the iteration count (default 20). The build is
//! release, as `cargo bench` always is; compare runs on the same machine and
//! with the same tessdata, since both construction and recognition depend on it.

use pdfextractor_ocr_engine::OcrEngine;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    let index = ((sorted.len() as f64 * fraction).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index]
}

fn report(label: &str, mut times: Vec<Duration>) {
    times.sort_unstable();
    let total: Duration = times.iter().sum();
    let mean = total / times.len() as u32;
    println!(
        "{label:28} n={:3}  min {:>8.2} ms  median {:>8.2} ms  p95 {:>8.2} ms  mean {:>8.2} ms",
        times.len(),
        times[0].as_secs_f64() * 1000.0,
        percentile(&times, 0.50).as_secs_f64() * 1000.0,
        percentile(&times, 0.95).as_secs_f64() * 1000.0,
        mean.as_secs_f64() * 1000.0,
    );
}

fn main() -> anyhow::Result<()> {
    let iterations: usize = std::env::var("BENCH_ITERS")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(20);

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let image_path = root.join("test-data/photos/01_flat.png");
    let image = std::fs::read(&image_path)?;

    println!(
        "engine_cost: {} iterations, image {} ({} bytes)",
        iterations,
        image_path.display(),
        image.len()
    );

    // Construction alone: each engine is dropped before the next starts, which
    // is what a per-document build does.
    let mut construction = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        let engine = OcrEngine::new("eng")?;
        construction.push(started.elapsed());
        black_box(&engine);
        drop(engine);
    }
    report("engine construction", construction.clone());

    // Recognition through one engine, after a warm-up that is not measured.
    let mut engine = OcrEngine::new("eng")?;
    let warmup = engine.recognize(&image)?;
    println!(
        "recognition warm-up: {} line(s), {} word(s)",
        warmup.lines.len(),
        warmup.words.len()
    );
    let mut recognition = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        let result = engine.recognize(&image)?;
        recognition.push(started.elapsed());
        black_box(result);
    }
    report("one-page recognition", recognition.clone());

    let mut construction_sorted = construction.clone();
    construction_sorted.sort_unstable();
    let mut recognition_sorted = recognition.clone();
    recognition_sorted.sort_unstable();
    let build = percentile(&construction_sorted, 0.50);
    let page = percentile(&recognition_sorted, 0.50);
    println!(
        "per-document estimate: build {:>8.2} ms + page {:>8.2} ms = {:>8.2} ms; \
         reuse saves {:.1}% of a one-page document",
        build.as_secs_f64() * 1000.0,
        page.as_secs_f64() * 1000.0,
        (build + page).as_secs_f64() * 1000.0,
        build.as_secs_f64() / (build + page).as_secs_f64() * 100.0
    );

    // The same documents read twice: once the old way (an engine built for
    // every one) and once with the engine a worker now keeps. This is the
    // before/after the command line's change is judged by.
    let documents = iterations.min(10);
    let mut fresh = Vec::with_capacity(documents);
    for _ in 0..documents {
        let started = Instant::now();
        let mut engine = OcrEngine::new("eng")?;
        let result = engine.recognize(&image)?;
        fresh.push(started.elapsed());
        black_box(result);
    }
    let mut reused = Vec::with_capacity(documents);
    let mut engine = OcrEngine::new("eng")?;
    for _ in 0..documents {
        let started = Instant::now();
        let result = engine.recognize(&image)?;
        reused.push(started.elapsed());
        black_box(result);
    }
    report("documents, fresh engine", fresh.clone());
    report("documents, worker engine", reused.clone());
    let mut fresh_sorted = fresh.clone();
    fresh_sorted.sort_unstable();
    let mut reused_sorted = reused.clone();
    reused_sorted.sort_unstable();
    let fresh_median = percentile(&fresh_sorted, 0.50);
    let reused_median = percentile(&reused_sorted, 0.50);
    println!(
        "per-document change: {:>8.2} ms before -> {:>8.2} ms after ({:+.1}%)",
        fresh_median.as_secs_f64() * 1000.0,
        reused_median.as_secs_f64() * 1000.0,
        (reused_median.as_secs_f64() / fresh_median.as_secs_f64() - 1.0) * 100.0
    );
    Ok(())
}
