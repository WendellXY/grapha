use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};

/// A progress bar for known-length operations.
pub fn bar(len: u64, msg: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg} [{bar:25.cyan/dim}] {pos}/{len}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
            .progress_chars("━╸─"),
    );
    pb.set_message(msg.to_string());
    pb
}

/// Print a completed step with elapsed time.
pub fn done(msg: &str, start: Instant) {
    let elapsed = start.elapsed();
    if elapsed.as_secs() >= 1 {
        eprintln!("  \x1b[32m✓\x1b[0m {} ({:.1}s)", msg, elapsed.as_secs_f64());
    } else {
        eprintln!("  \x1b[32m✓\x1b[0m {} ({}ms)", msg, elapsed.as_millis());
    }
}

/// Print a summary line.
pub fn summary(msg: &str) {
    eprintln!("\x1b[1m{}\x1b[0m", msg);
}
