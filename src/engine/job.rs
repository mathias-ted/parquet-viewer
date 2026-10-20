//! A cancellable background task with a progress counter (index builds, sort, export).
use crate::prelude::*;

pub(crate) struct Job {
    label: std::sync::Mutex<String>,
    pub(crate) done: AtomicUsize,
    pub(crate) total: AtomicUsize,
    pub(crate) cancel: AtomicBool,
    pub(crate) skipped: AtomicUsize, // row groups skipped thanks to file statistics
    pub(crate) indeterminate: AtomicBool, // a phase with no countable progress (sorting, saving, loading)
    ctx: egui::Context,
}

impl Job {
    pub(crate) fn new(label: &str, total: usize, ctx: &egui::Context) -> Self {
        Self { label: std::sync::Mutex::new(label.to_string()), done: AtomicUsize::new(0), total: AtomicUsize::new(total), cancel: AtomicBool::new(false), skipped: AtomicUsize::new(0), indeterminate: AtomicBool::new(false), ctx: ctx.clone() }
    }
    pub(crate) fn label(&self) -> String { self.label.lock().map(|l| l.clone()).unwrap_or_default() }
    /// Switch to a new phase without a progress count (the bar animates instead).
    pub(crate) fn set_phase(&self, text: &str) {
        if let Ok(mut l) = self.label.lock() { *l = text.to_string(); }
        self.indeterminate.store(true, Relaxed);
        self.ctx.request_repaint();
    }
    pub(crate) fn cancelled(&self) -> bool { self.cancel.load(Relaxed) }
    /// One unit of work finished; wakes the UI so the progress bar moves.
    pub(crate) fn add(&self, n: usize) { self.done.fetch_add(n, Relaxed); self.ctx.request_repaint(); }
    pub(crate) fn tick(&self) { self.add(1); }
    pub(crate) fn frac(&self) -> f32 { (self.done.load(Relaxed) as f32 / self.total.load(Relaxed).max(1) as f32).min(1.0) }
}
