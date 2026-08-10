use std::path::PathBuf;
use std::time::Instant;

use u1_three_mf::analyze_project;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: analyze_benchmark <project.3mf>")?;
    let started = Instant::now();
    let analysis = analyze_project(&path)?;
    let elapsed = started.elapsed();

    println!(
        "elapsed_seconds={:.6} bytes={} entries={} vertices={} triangles={} objects={} plates={} sha256={}",
        elapsed.as_secs_f64(),
        analysis.input.byte_size,
        analysis.archive.entry_count,
        analysis.summary.vertex_count,
        analysis.summary.triangle_count,
        analysis.summary.object_count,
        analysis.summary.plate_count,
        analysis.input.sha256,
    );
    Ok(())
}
