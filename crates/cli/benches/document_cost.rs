//! What a document of each size costs to convert, and what parallelism buys.
//!
//! ```text
//! cargo bench -p pdfextractor-cli --bench document_cost
//! ```
//!
//! Three fixtures stand in for the sizes a run meets: a one-page brief, a
//! three-page PDF and a fourteen-page paper. Each is timed on its own, then as
//! many copies as there are threads — first back to back on one thread, then
//! across a rayon pool of that many threads. That is the shape of `--parallel
//! 1` against `--parallel N` over N input documents; the bench calls the
//! pipeline directly so process startup is not in the numbers.
//!
//! `BENCH_ITERS` overrides the single-document samples (default 10), and
//! `BENCH_BATCH_ITERS` the batch samples (default 5). Pictures are described
//! rather than written (`image_base` is `None`), so the cost measured is the
//! reading's, not the disk's. The build is release, as `cargo bench` always is;
//! compare runs on the same machine, since the thread count is the machine's.

use pdfextractor_core::DoclingDocument;
use pdfextractor_pipeline::{build_document, ConvertOptions};
use rayon::prelude::*;
use std::hint::black_box;
use std::path::{Path, PathBuf};
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
        "{label:40} n={:3}  min {:>8.2} ms  median {:>8.2} ms  p95 {:>8.2} ms  mean {:>8.2} ms",
        times.len(),
        times[0].as_secs_f64() * 1000.0,
        percentile(&times, 0.50).as_secs_f64() * 1000.0,
        percentile(&times, 0.95).as_secs_f64() * 1000.0,
        mean.as_secs_f64() * 1000.0,
    );
}

fn median(times: &[Duration]) -> Duration {
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    percentile(&sorted, 0.50)
}

/// One conversion with the reading's defaults: born-digital pages are read
/// from their own text, and no picture is written.
fn convert(path: &Path) -> DoclingDocument {
    let options = ConvertOptions {
        ocr_lang: "eng",
        ..ConvertOptions::default()
    };
    build_document(path, &options).expect("a fixture converts")
}

fn main() -> anyhow::Result<()> {
    let iterations: usize = std::env::var("BENCH_ITERS")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(10);
    let batch_iterations: usize = std::env::var("BENCH_BATCH_ITERS")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(5);

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixtures = [
        ("small", "test-data/brief.docx"),
        ("medium", "test-data/real/basicapi.pdf"),
        ("large", "test-data/real/tracemonkey.pdf"),
    ];
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    println!(
        "document_cost: {threads} thread(s), {iterations} single-document and \
         {batch_iterations} batch round(s)"
    );
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()?;

    for (size, relative) in fixtures {
        let path = root.join(relative);
        let bytes = std::fs::metadata(&path)?.len();

        // A warm-up conversion: the first read of a file pays the page cache,
        // and the pages and items it yields are what the label describes.
        let sample = convert(&path);
        println!(
            "{size}: {} ({} bytes, {} page(s), {} text item(s), {} picture(s))",
            relative,
            bytes,
            sample.pages.len(),
            sample.texts.len(),
            sample.pictures.len()
        );

        let mut single = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let started = Instant::now();
            let document = convert(&path);
            single.push(started.elapsed());
            black_box(document);
        }
        report(&format!("{size}, one document"), single);

        let copies = vec![path.as_path(); threads];
        let mut serial = Vec::with_capacity(batch_iterations);
        for _ in 0..batch_iterations {
            let started = Instant::now();
            let documents: Vec<_> = copies.iter().map(|copy| convert(copy)).collect();
            serial.push(started.elapsed());
            black_box(documents);
        }
        report(
            &format!("{size}, {threads} documents serial"),
            serial.clone(),
        );

        let mut parallel = Vec::with_capacity(batch_iterations);
        for _ in 0..batch_iterations {
            let started = Instant::now();
            let documents: Vec<_> = pool.install(|| {
                copies
                    .par_iter()
                    .map(|copy| convert(copy))
                    .collect::<Vec<_>>()
            });
            parallel.push(started.elapsed());
            black_box(documents);
        }
        report(
            &format!("{size}, {threads} documents parallel"),
            parallel.clone(),
        );

        let serial_median = median(&serial);
        let parallel_median = median(&parallel);
        println!(
            "{size:6} batch: {:>8.2} ms serial -> {:>8.2} ms parallel ({:.2}x on {threads} threads, {} documents)",
            serial_median.as_secs_f64() * 1000.0,
            parallel_median.as_secs_f64() * 1000.0,
            serial_median.as_secs_f64() / parallel_median.as_secs_f64(),
            threads
        );
    }
    Ok(())
}
