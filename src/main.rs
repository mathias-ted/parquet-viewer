//! Parquet viewer: footer-only open, row-group pruning, parallel scans, indexes,
//! top-k sort, SQL box and export. See README.md for the module map.
// Normal builds keep the console, so crash messages are visible. The shareable build hides it:
//   cargo build --release --features hide-console
#![cfg_attr(all(not(debug_assertions), feature = "hide-console"), windows_subsystem = "windows")]

mod data;
mod engine;
mod prelude;
mod ui;
#[cfg(test)]
mod tests;

fn main() -> eframe::Result {
    // Any crash is written to a log file, because the release build has no console window.
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("{info}\n{}", std::backtrace::Backtrace::force_capture());
        let _ = std::fs::write(std::env::temp_dir().join("parquet-viewer-crash.log"), &msg);
        eprintln!("{msg}");
    }));
    ui::app::run()
}
