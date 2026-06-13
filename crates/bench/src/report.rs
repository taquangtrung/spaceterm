//! The benchmark result, aggregated over several runs and formatted identically
//! for both backends.

use serde::Serialize;

// ========================================================================
// Data Structures
// ========================================================================

/// One backend's result over `runs` measured passes of the shared corpus.
/// Throughput is summarized across runs; the frame p99 is the median of the
/// per-run p99s. Serialized verbatim for `--json` tracking.
#[derive(Serialize)]
pub struct BenchReport {
    /// The renderer that actually ran (e.g. `wgpu+glyphon`, `webgl`, `canvas`).
    pub backend: String,
    pub frame_p99_ms_median: f64,
    /// The mode the user selected (`glyphon` or `webgl`).
    pub mode: String,
    pub runs: usize,
    pub throughput_mb_s_max: f64,
    pub throughput_mb_s_median: f64,
    pub throughput_mb_s_min: f64,
    pub total_bytes: usize,
}

// ========================================================================
// BenchReport
// ========================================================================

impl BenchReport {
    /// Aggregate per-run measurements into one report.
    pub fn from_runs(
        mode: &str,
        backend: &str,
        total_bytes: usize,
        throughputs: &[f64],
        frame_p99s: &[f64],
    ) -> Self {
        let (median, min, max) = summarize(throughputs);
        let (frame_p99_median, _, _) = summarize(frame_p99s);
        Self {
            backend: backend.to_string(),
            frame_p99_ms_median: frame_p99_median,
            mode: mode.to_string(),
            runs: throughputs.len(),
            throughput_mb_s_max: max,
            throughput_mb_s_median: median,
            throughput_mb_s_min: min,
            total_bytes,
        }
    }

    pub fn print(&self) {
        let mib = self.total_bytes as f64 / (1024.0 * 1024.0);
        println!("=== spaceterm-bench: {} ===", self.mode);
        println!("backend          {}", self.backend);
        println!("corpus           {mib:.2} MiB ({} bytes)", self.total_bytes);
        println!("runs             {}", self.runs);
        println!(
            "throughput       {:.3} MiB/s (median; min {:.3}, max {:.3})",
            self.throughput_mb_s_median, self.throughput_mb_s_min, self.throughput_mb_s_max
        );
        println!(
            "frame p99        {:.2} ms (median)",
            self.frame_p99_ms_median
        );
    }

    pub fn print_json(&self) {
        match serde_json::to_string(self) {
            Ok(line) => println!("{line}"),
            Err(error) => eprintln!("spaceterm-bench: serialize report: {error}"),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Median, min, and max of `values`. Returns zeros when empty.
fn summarize(values: &[f64]) -> (f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("measurements are finite"));
    let median = sorted[sorted.len() / 2];
    (median, sorted[0], sorted[sorted.len() - 1])
}
