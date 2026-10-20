//! The egui application: state, toolbar, table, panels.
use crate::prelude::*;

#[derive(Default)]
struct App {
    t: Option<Arc<Table>>,
    err: String,
    q: String,
    equals: bool,
    col: Option<usize>,
    view: Option<Vec<u64>>, // matched row ids; None = browse all rows
    scan: Option<Scan>,
    cache: HashMap<(usize, u64), Vec<Vec<String>>>,
    pending: HashSet<(usize, u64)>,
    blk_tx: Option<(Sender<((usize, u64), Vec<Vec<String>>)>, Receiver<((usize, u64), Vec<Vec<String>>)>)>,
    opened_ms: f64,
    top: u64,  // first visible row (virtual scroll, no pixel limit)
    vis: u64,  // rows that fit on screen
    acc: f32,  // wheel accumulator
    idx: Option<Arc<TriIdx>>,
    ibuild: Option<(Receiver<Option<TriIdx>>, Arc<Job>)>,
    exact: HashMap<usize, Arc<ExactIdx>>,
    ebuild: Option<(Receiver<Option<ExactIdx>>, Arc<Job>, usize)>,
    note: String,
    sorting: Option<(Receiver<Option<Vec<u64>>>, Instant, usize, bool, Option<Arc<Vec<u64>>>, Arc<Job>)>,
    sql_open: bool,
    sql_text: String,
    sql_dir: Option<PathBuf>,
    sql_rx: Option<Receiver<Result<SqlResult, String>>>,
    sql_view: Option<SqlResult>,
    sql_msg: String,
    exp_rx: Option<(Receiver<Result<usize, String>>, Arc<Job>)>,
    table_rect: Option<egui::Rect>, // where the data table is on screen (from the previous frame)
    assoc_open: bool,
    assoc_msg: String,
    schema_open: bool,
    schema_info: Option<SchemaInfo>,
    hx: f32, // horizontal scroll position of the table (so the arrow keys can move it)
    sql_inflight: Option<(PathBuf, String)>, // query being run
    sql_ran: Option<(PathBuf, String)>,      // query that produced the result on screen (re-run to export all rows)
    rerun_search: bool, // an exact index was loading in the background; search again when it is ready
    job: Option<Arc<Job>>, // the one running background task (index build, sort, export)
    exp_open: bool,
    exp_fmt: ExportFmt,
    exp_sel: Vec<bool>,
    sort_state: Option<(usize, bool)>, // (column, descending) of the active header sort
    base: Option<Arc<Vec<u64>>>,       // search results being sorted (None = whole data)
}

impl App {
    fn open_path(&mut self, p: PathBuf) {
        let t0 = Instant::now();
        match open(p) {
            Ok(t) => { self.t = Some(Arc::new(t)); self.opened_ms = t0.elapsed().as_secs_f64() * 1e3; self.err.clear(); self.col = None; self.schema_info = None; self.sort_state = None; if let Some(j) = self.job.take() { j.cancel.store(true, Relaxed); } self.rerun_search = false; self.ibuild = None; self.exact.clear(); self.ebuild = None; self.sql_dir = None; self.idx = self.t.as_deref().and_then(load_idx).map(Arc::new); self.clear(); }
            Err(e) => self.err = format!("Cannot open: {e}"),
        }
    }
    /// Reserve the single background-task slot. None (with a note) if a task is already running.
    fn start_job(&mut self, label: &str, total: usize, ctx: &egui::Context) -> Option<Arc<Job>> {
        if self.job.is_some() { self.note = "A task is already running: wait for it or press Cancel".into(); return None; }
        let j = Arc::new(Job::new(label, total, ctx));
        self.job = Some(j.clone());
        Some(j)
    }
    /// A task's result arrived: free the slot (only if it is still this task's) and report whether it was cancelled.
    fn finish_job(&mut self, j: &Arc<Job>) -> bool {
        if self.job.as_ref().map_or(false, |c| Arc::ptr_eq(c, j)) { self.job = None; }
        j.cancelled()
    }
    fn drop_sorting(&mut self) {
        if self.sorting.take().is_some() { if let Some(j) = self.job.take() { j.cancel.store(true, Relaxed); } }
    }
    fn build_index(&mut self, ctx: &egui::Context) {
        let Some(t) = self.t.clone() else { return };
        let (need, free) = (text_index_mem_need(&t), free_memory());
        if need > free {
            self.note = format!("Text index for this data needs about {:.1} GB of free memory but only {:.1} GB is free. Close other programs, or skip it: searching still works without it.", need as f64 / 1e9, free as f64 / 1e9);
            return;
        }
        let Some(job) = self.start_job("Building text index", t.num_rgs(), ctx) else { return };
        let (tx, rx) = channel();
        let (j2, c2) = (job.clone(), ctx.clone());
        std::thread::spawn(move || {
            let ix = build_idx(&t, &j2);
            if let Some(i) = &ix { save_idx(&t, i); }
            let _ = tx.send(ix);
            c2.request_repaint();
        });
        self.ibuild = Some((rx, job));
    }
    /// Load a saved exact index on a worker thread. false = not started (not enough memory, or busy): search falls back to a scan.
    fn start_exact_load(&mut self, t: &Arc<Table>, c: usize, ctx: &egui::Context) -> bool {
        let (need, free) = (exact_mem_need(t), free_memory());
        if need > free {
            self.note = format!("Exact index needs about {:.1} GB of free memory but only {:.1} GB is free, so a normal scan is used", need as f64 / 1e9, free as f64 / 1e9);
            return false;
        }
        let Some(job) = self.start_job(&format!("Loading exact index ({})", t.names[c]), 1, ctx) else { return false };
        job.set_phase(&format!("Loading exact index ({})...", t.names[c]));
        let (tx, rx) = channel();
        let (t2, c2) = (t.clone(), ctx.clone());
        std::thread::spawn(move || {
            let ix = load_exact(&t2, c);
            if ix.is_none() { let _ = std::fs::remove_file(exact_path(&t2, c)); } // unreadable or stale: allow rebuilding
            let _ = tx.send(ix);
            c2.request_repaint();
        });
        self.ebuild = Some((rx, job, c));
        self.rerun_search = true;
        true
    }
    fn build_exact_index(&mut self, c: usize, ctx: &egui::Context) {
        let Some(t) = self.t.clone() else { return };
        let (need, free) = (exact_mem_need(&t), free_memory());
        if need > free {
            self.note = format!("Exact index for {} rows needs about {:.1} GB of free memory but only {:.1} GB is free. Close other programs, or use text search or SQL for this data.", t.total(), need as f64 / 1e9, free as f64 / 1e9);
            return;
        }
        let Some(job) = self.start_job(&format!("Building exact index ({})", t.names[c]), t.num_rgs(), ctx) else { return };
        let (tx, rx) = channel();
        let (j2, c2) = (job.clone(), ctx.clone());
        std::thread::spawn(move || {
            let ix = build_exact(&t, c, &j2);
            if let Some(i) = &ix { j2.set_phase("Saving exact index to disk..."); save_exact(&t, c, i); }
            let _ = tx.send(ix);
            c2.request_repaint();
        });
        self.ebuild = Some((rx, job, c));
    }
    fn sort(&mut self, desc: bool, ctx: &egui::Context) {
        let Some(t) = self.t.clone() else { return };
        let Some(c) = self.col else { self.note = "Pick a column to sort by".into(); return };
        if self.scan.as_ref().map_or(false, |s| s.done.is_none()) { self.note = "Wait for the search to finish before sorting".into(); return; }
        let Some(job) = self.start_job("Sorting", t.num_rgs(), ctx) else { return };
        // first sort after a search: sort within those results; later clicks keep the same set
        let base = if self.sort_state.is_none() { self.view.as_ref().map(|v| Arc::new(v.clone())) } else { self.base.clone() };
        self.note = format!("sorting by {}…", t.names[c]);
        let subset = base.clone();
        let k = subset.as_ref().map_or(1000, |s| s.len().clamp(1, 100_000));
        let (tx, rx) = channel();
        let (j2, ctx2) = (job.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = build_topk(&t, c, k, desc, subset.as_deref().map(|v| v.as_slice()), &j2).ok();
            let _ = tx.send(r);
            ctx2.request_repaint();
        });
        self.sorting = Some((rx, Instant::now(), c, desc, base, job));
    }
    fn do_export(&mut self, ctx: &egui::Context) {
        let cols: Vec<usize> = self.exp_sel.iter().enumerate().filter(|(_, &on)| on).map(|(i, _)| i).collect();
        if cols.is_empty() { self.note = "Select at least one column to export".into(); return; }
        if self.sql_view.is_none() && self.view.is_none() { self.note = "Nothing to export: run a search, sort or SQL query first".into(); return; }
        if self.job.is_some() { self.note = "A task is already running: wait for it or press Cancel".into(); return; }
        let fmt = self.exp_fmt;
        let Some(path) = rfd::FileDialog::new().add_filter(fmt.ext(), &[fmt.ext()]).set_file_name(format!("export.{}", fmt.ext())).save_file() else { return };
        // A result cut at 100,000 rows: run the query again and stream every row to the file.
        if self.sql_view.as_ref().map_or(false, |r| r.truncated) && self.sql_ran.is_some() {
            let Some((src, q)) = self.sql_ran.clone() else { return };
            let Some(job) = self.start_job("Exporting SQL results", 1, ctx) else { return };
            job.set_phase("Exporting SQL results...");
            let (tx, rx) = channel();
            let (j2, c2) = (job.clone(), ctx.clone());
            let started = std::thread::Builder::new().name("sql-export".into()).stack_size(64 * 1024 * 1024).spawn(move || {
                let _ = tx.send(export_sql_stream(src, q, &path, fmt, &cols, &j2));
                c2.request_repaint();
            });
            if started.is_err() { self.finish_job(&job); self.note = "could not start the export".into(); return; }
            self.exp_rx = Some((rx, job));
            return;
        }
        if let Some(r) = &self.sql_view {
            let m = match export_sql(r, &path, fmt, &cols) { Ok(n) => format!("exported {n} rows to {}", path.display()), Err(e) => format!("export failed: {e}") };
            self.sql_msg = m.clone();
            self.note = m;
            return;
        }
        let (Some(t), Some(ids)) = (self.t.clone(), self.view.clone()) else { return };
        self.note = format!("exporting {} rows…", ids.len());
        let Some(job) = self.start_job("Exporting", 1, ctx) else { return };
        let (tx, rx) = channel();
        let (j2, c2) = (job.clone(), ctx.clone());
        std::thread::spawn(move || { let _ = tx.send(export_ids(&t, &ids, &path, fmt, &cols, &j2).map_err(|e| e.to_string())); c2.request_repaint(); });
        self.exp_rx = Some((rx, job));
    }
    fn run_sql_query(&mut self, ctx: &egui::Context) {
        let Some(path) = self.sql_dir.clone().or_else(|| self.t.as_ref().map(|t| t.path.clone())) else { self.sql_msg = "Open a file or folder first".into(); return };
        let sql = self.sql_text.clone();
        self.sql_inflight = Some((path.clone(), sql.clone()));
        let (tx, rx) = channel();
        let c2 = ctx.clone();
        // A big stack: the SQL engine plans queries with deep recursion, and Rust's default 2 MB thread stack can overflow.
        let started = std::thread::Builder::new().name("sql".into()).stack_size(64 * 1024 * 1024)
            .spawn(move || { let _ = tx.send(run_sql(path, sql)); c2.request_repaint(); });
        if let Err(e) = started { self.sql_msg = format!("could not start the query: {e}"); return; }
        self.sql_rx = Some(rx);
        self.sql_msg = "running…".into();
    }
    fn count(&self) -> u64 {
        if let Some(r) = &self.sql_view { return r.rows.len() as u64; }
        self.view.as_ref().map_or_else(|| self.t.as_ref().map_or(0, |t| t.total()), |v| v.len() as u64)
    }
    fn clear(&mut self) {
        self.top = 0;
        self.note.clear();
        self.drop_sorting();
        self.sql_view = None;
        self.sort_state = None;
        self.base = None;
        if let Some(s) = &self.scan { s.cancel.store(true, Relaxed); }
        self.scan = None; self.view = None; self.cache.clear(); self.pending.clear();
        self.blk_tx = Some(channel());
    }
    fn search(&mut self, ctx: &egui::Context) {
        let Some(t) = self.t.clone() else { return };
        if self.q.is_empty() { return; }
        self.note.clear();
        self.drop_sorting();
        self.sql_view = None;
        self.sort_state = None;
        self.base = None;
        if self.equals && std::env::var_os("PQV_NOEXACT").is_none() { // debug switch: PQV_NOEXACT=1 ignores the exact index
            if let Some(c) = self.col {
                let ix = self.exact.get(&c).cloned();
                if ix.is_none() && self.ebuild.is_none() && exact_path(&t, c).exists() && self.start_exact_load(&t, c, ctx) { return; }
                if let Some(ix) = ix { // exact index: binary search, no file reads
                    if let Some(s) = &self.scan { s.cancel.store(true, Relaxed); }
                    let t0 = Instant::now();
                    let hits = exact_lookup(&ix, &self.q);
                    self.note = format!("{} matches via exact index in {:.2} ms", hits.len(), t0.elapsed().as_secs_f64() * 1e3);
                    self.top = 0; self.scan = None; self.view = Some(hits);
                    return;
                }
            }
        }
        if let Some(s) = &self.scan { s.cancel.store(true, Relaxed); }
        let cols = self.col.map(|c| vec![c]).unwrap_or_else(|| (0..t.names.len()).collect());
        let range = if self.equals { None } else {
            self.col.filter(|&c| t.flat && is_numeric(t.schema().field(c).data_type())).and_then(|_| parse_range(&self.q))
        };
        self.top = 0;
        self.view = Some(vec![]);
        self.scan = Some(start_scan(&t, cols, self.q.clone(), self.equals, range, self.idx.clone(), ctx.clone()));
    }
    fn pump(&mut self) {
        if let Some(s) = &mut self.scan {
            for m in s.rx.try_iter() {
                match m { Msg::Hits(h) => self.view.get_or_insert_with(Vec::new).extend(h),
                          Msg::Done => { s.done = Some(s.t0.elapsed().as_secs_f64()); if let Some(v) = &mut self.view { v.sort_unstable(); } } }
            }
        }
        let got = self.ibuild.as_ref().and_then(|(rx, j)| rx.try_recv().ok().map(|r| (r, j.clone())));
        if let Some((r, j)) = got {
            let cancelled = self.finish_job(&j);
            self.ibuild = None;
            match r {
                Some(ix) => self.idx = Some(Arc::new(ix)),
                None => self.note = if cancelled { "text index cancelled".into() } else { "text index failed".into() },
            }
        }
        let got = self.exp_rx.as_ref().and_then(|(rx, j)| rx.try_recv().ok().map(|r| (r, j.clone())));
        if let Some((r, j)) = got {
            self.finish_job(&j);
            self.exp_rx = None;
            self.note = match r {
                Ok(n) => format!("exported {n} rows"),
                Err(e) if e == "cancelled" => "export cancelled".into(),
                Err(e) => format!("export failed: {e}"),
            };
        }
        if let Some(r) = self.sql_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.sql_rx = None;
            match r {
                Ok(res) => {
                    self.sql_ran = self.sql_inflight.take();
                    self.sql_msg = format!("{} rows{} in {:.0} ms", res.rows.len(), if res.truncated { format!(" (first {SQL_MAX_ROWS} shown)") } else { String::new() }, res.ms);
                    self.top = 0;
                    self.sql_view = Some(res);
                }
                Err(e) => {
                    let hint = if e.contains("Resources exhausted") { " (the query needs more memory than is available: add a WHERE or a LIMIT, or close other programs)" } else { "" };
                    self.sql_msg = format!("SQL error: {e}{hint}");
                }
            }
        }
        let got = self.sorting.as_ref().and_then(|(rx, t0, c, d, b, j)| rx.try_recv().ok().map(|r| (r, *t0, *c, *d, b.clone(), j.clone())));
        if let Some((r, t0, c, d, b, j)) = got {
            let cancelled = self.finish_job(&j);
            self.sorting = None;
            match r {
                Some(ids) => {
                    self.base = b;
                    self.sort_state = Some((c, d));
                    self.sql_view = None;
                    let name = self.t.as_ref().map_or(String::new(), |t| t.names[c].clone());
                    let scope = if self.base.is_some() { "of the search results" } else { "of all rows" };
                    let sk = j.skipped.load(Relaxed);
                    let skip_txt = if sk > 0 { format!(" · skipped {sk} of {} row groups using file statistics", self.t.as_ref().map_or(0, |t| t.num_rgs())) } else { String::new() };
                    self.note = format!("top {} {} by {} {} in {:.0} ms{}", ids.len(), scope, name, if d { "descending" } else { "ascending" }, t0.elapsed().as_secs_f64() * 1e3, skip_txt);
                    self.top = 0;
                    self.view = Some(ids);
                }
                None => self.note = if cancelled { "sort cancelled".into() } else { "cannot sort this column type".into() },
            }
        }
        let got = self.ebuild.as_ref().and_then(|(rx, j, c)| rx.try_recv().ok().map(|r| (*c, r, j.clone())));
        if let Some((c, r, j)) = got {
            let cancelled = self.finish_job(&j);
            self.ebuild = None;
            match r {
                Some(ix) => { self.exact.insert(c, Arc::new(ix)); }
                None => self.note = if cancelled { "exact index cancelled".into() } else { "exact index failed (read error or over 4G rows)".into() },
            }
        }
        if let Some((_, rx)) = &self.blk_tx {
            for (k, rows) in rx.try_iter() { self.pending.remove(&k); if self.cache.len() > 200 { self.cache.clear(); } self.cache.insert(k, rows); }
        }
    }
    /// Start loading a block unless it is cached, in flight, or `limit` loads are already running.
    fn request(&mut self, k: (usize, u64), ctx: &egui::Context, limit: usize) {
        if self.cache.contains_key(&k) || self.pending.contains(&k) || self.pending.len() >= limit { return; }
        let (Some(t), Some((tx, _))) = (self.t.clone(), &self.blk_tx) else { return };
        self.pending.insert(k);
        let (tx, ctx) = (tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let rows = load_block(&t, k.0, k.1).unwrap_or_default(); // on error: empty, so the request never stays stuck
            let _ = tx.send((k, rows));
            ctx.request_repaint();
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if let Some(p) = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone())) { self.open_path(p); }
        self.pump();
        // a column picked in a previous file can be out of range for the table that is open now
        if let (Some(c), Some(t)) = (self.col, &self.t) { if c >= t.names.len() { self.col = None; } }
        if self.rerun_search && self.ebuild.is_none() { self.rerun_search = false; self.search(ctx); } // exact index finished loading
        let n = self.count();
        let (dy, pg) = ctx.input(|i| (i.smooth_scroll_delta.y, i.key_pressed(egui::Key::PageDown) as i64 - i.key_pressed(egui::Key::PageUp) as i64));
        // The wheel scrolls the table only when the pointer is over the table itself, not over a dropdown,
        // a window or a toolbar; Page Up/Down are ignored while typing in a text box.
        let over_table = ctx.pointer_hover_pos().map_or(false, |p| {
            self.table_rect.map_or(false, |r| r.contains(p)) && ctx.layer_id_at(p).map_or(true, |l| l.order == egui::Order::Background)
        });
        let dy = if over_table { dy } else { 0.0 };
        let kb_free = !ctx.wants_keyboard_input() && !ctx.memory(|m| m.any_popup_open());
        let pg = if kb_free { pg } else { 0 };
        let (up, down, left, right, home, end, shift) = ctx.input(|i| (
            i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::ArrowLeft),
            i.key_pressed(egui::Key::ArrowRight), i.key_pressed(egui::Key::Home), i.key_pressed(egui::Key::End), i.modifiers.shift));
        let step = if kb_free { (down as i64 - up as i64) * if shift { 10 } else { 1 } } else { 0 }; // Shift = 10 rows
        if kb_free { self.hx = (self.hx + 80.0 * (right as i8 - left as i8) as f32).max(0.0); }
        self.acc += dy;
        let d = (self.acc / 20.0).trunc();
        self.acc -= d * 20.0;
        let d = d * (1.0 + (d.abs() / 5.0).min(30.0)); // accelerate fast wheel spins
        let max = n.saturating_sub(self.vis);
        self.top = (self.top as i64 - d as i64 + pg * self.vis as i64 + step).clamp(0, max as i64) as u64;
        if kb_free && home { self.top = 0; }
        if kb_free && end { self.top = max; }
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Open…").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("parquet", &["parquet", "pq"]).pick_file() { self.open_path(p); }
                }
                if ui.button("Open folder…").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() { self.open_path(p); } // every .parquet/.pq inside, as one table
                }
                if ui.button("Export…").clicked() { self.exp_open = !self.exp_open; }
                if ui.button("Schema").clicked() { self.schema_open = !self.schema_open; }
                if ui.button("Default app…").clicked() { self.assoc_open = !self.assoc_open; }
                if ui.button("SQL").clicked() {
                    self.sql_open = !self.sql_open;
                    if self.sql_text.is_empty() { self.sql_text = "SELECT * FROM data LIMIT 1000".into(); }
                }
                if let Some(t) = self.t.clone() {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.q).hint_text("search…").desired_width(220.0));
                    egui::ComboBox::from_id_salt("col")
                        .selected_text(self.col.map(|c| t.names[c].clone()).unwrap_or("All columns".into()))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.col, None, "All columns");
                            for (i, n) in t.names.iter().enumerate() { ui.selectable_value(&mut self.col, Some(i), n); }
                        });
                    ui.checkbox(&mut self.equals, "equals");
                    if ui.button("Search").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) { self.search(ctx); }
                    if ui.button("Cancel").clicked() {
                        if let Some(s) = &self.scan { s.cancel.store(true, Relaxed); }
                        if let Some(j) = &self.job { j.cancel.store(true, Relaxed); }
                    }
                    if ui.button("Clear").clicked() { self.q.clear(); self.clear(); }
                    if self.idx.is_none() && self.ibuild.is_none() && ui.button("Build text index").clicked() { self.build_index(ctx); }
                    if let Some(c) = self.col {
                        if !self.exact.contains_key(&c) && self.ebuild.is_none() && !exact_path(&t, c).exists()
                            && ui.button(format!("Build exact index ({})", t.names[c])).clicked() { self.build_exact_index(c, ctx); }
                    }
                }
            });
            let status = match (&self.t, &self.scan) {
                (None, _) => if self.err.is_empty() { "Open or drop a .parquet file".to_string() } else { self.err.clone() },
                (Some(_), None) if !self.note.is_empty() => self.note.clone(),
                (Some(t), None) => format!("{} · {}{} rows · {} row groups · {} cols · footer read in {:.1} ms",
                    t.path.file_name().map_or(String::new(), |n| n.to_string_lossy().to_string()), if t.files.len() > 1 { format!("{} files · ", t.files.len()) } else { String::new() }, t.total(), t.num_rgs(), t.names.len(), self.opened_ms),
                (Some(t), Some(s)) => format!("{} matches · {} scanned, {} skipped via stats of {} row groups · {} · CPU decode {:.1}s, match {:.1}s",
                    self.view.as_ref().map_or(0, |v| v.len()), s.scanned.load(Relaxed), s.skipped.load(Relaxed),
                    t.num_rgs(), match s.done { Some(d) => format!("{:.0} ms", d * 1e3), None => "scanning…".into() },
                    s.dec.load(Relaxed) as f64 / 1e9, s.mat.load(Relaxed) as f64 / 1e9),
            };
            let nrg = self.t.as_ref().map_or(0, |t| t.num_rgs());
            let isfx = match (&self.idx, &self.ibuild) {
                (Some(i), _) => format!(" · text index ready ({} string cols)", i.cols.len()),
                (None, Some(_)) => String::new(), // shown in the progress bar
                _ => String::new(),
            };
            let dsfx = match (self.col, &self.t) {
                (Some(c), Some(t)) => format!(" · selected col fully dictionary-encoded in {}/{} row groups", (0..nrg).filter(|&g| pure_dict(t.rg_meta(g).column(c))).count(), nrg),
                _ => String::new(),
            };
            let esfx = match (&self.ebuild, self.exact.len()) {
                (Some(_), _) => String::new(), // shown in the progress bar
                (None, 0) => String::new(),
                (None, k) => format!(" · exact index on {k} col(s)"),
            };
            ui.label(status + &isfx + &esfx + &dsfx);
            let scan_run = self.scan.as_ref().filter(|s| s.done.is_none()).map(|s| s.scanned.load(Relaxed) + s.skipped.load(Relaxed));
            if self.job.is_some() || scan_run.is_some() {
                ui.horizontal(|ui| {
                    if let Some(j) = self.job.clone() {
                        let ind = j.indeterminate.load(Relaxed);
                        let txt = if ind { j.label() } else { format!("{}  {}/{}", j.label(), j.done.load(Relaxed), j.total.load(Relaxed)) };
                        ui.add(egui::ProgressBar::new(if ind { 1.0 } else { j.frac() }).animate(ind).desired_width(420.0).text(txt));
                        if ui.button("Cancel task").clicked() { j.cancel.store(true, Relaxed); }
                    } else if let Some(d) = scan_run {
                        ui.add(egui::ProgressBar::new(d as f32 / nrg.max(1) as f32).desired_width(320.0).text(format!("Searching  {d}/{nrg} row groups")));
                        if ui.button("Cancel search").clicked() { if let Some(s) = &self.scan { s.cancel.store(true, Relaxed); } }
                    }
                });
            }
        });
        if self.sql_open {
            egui::TopBottomPanel::bottom("sql").resizable(true).min_height(100.0).show(ctx, |ui| {
                let src = self.sql_dir.as_ref().or_else(|| self.t.as_ref().map(|t| &t.path)).map(|p| p.display().to_string()).unwrap_or("(nothing open)".into());
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("SQL — table `data` = {src}"));
                    if ui.button("Run").clicked() { self.run_sql_query(ctx); }
                    if self.sql_dir.is_some() && ui.button("Use open file").clicked() { self.sql_dir = None; }
                    ui.label(self.sql_msg.clone());
                });
                ui.add(egui::TextEdit::multiline(&mut self.sql_text).code_editor().desired_rows(4).desired_width(f32::INFINITY));
            });
        }
        egui::SidePanel::right("sb").exact_width(26.0).show(ctx, |ui| {
            let h = ui.available_height();
            let vis = (((h - 50.0) / 20.0).floor().max(1.0)) as u64; // header + horizontal scroll bar
            if vis != self.vis { self.vis = vis; ctx.request_repaint(); }
            let max = n.saturating_sub(self.vis);
            if max > 0 {
                let mut v = (max - self.top.min(max)) as f64; // inverted: top of slider = first row
                ui.spacing_mut().slider_width = (h - 10.0).max(20.0);
                if ui.add(egui::Slider::new(&mut v, 0.0..=max as f64).vertical().show_value(false)).changed() {
                    self.top = max - (v.round() as u64).min(max);
                }
            }
        });
        if self.exp_open {
            let names: Vec<String> = match &self.sql_view {
                Some(r) => r.cols.clone(),
                None => self.t.as_ref().map(|t| t.names.clone()).unwrap_or_default(),
            };
            if self.exp_sel.len() != names.len() { self.exp_sel = vec![true; names.len()]; }
            let (mut open, mut go) = (true, false);
            egui::Window::new("Export current results").open(&mut open).collapsible(false).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Format:");
                    ui.radio_value(&mut self.exp_fmt, ExportFmt::Csv, "CSV");
                    ui.radio_value(&mut self.exp_fmt, ExportFmt::Txt, "TXT (colon-separated)");
                    ui.radio_value(&mut self.exp_fmt, ExportFmt::Json, "JSON");
                });
                ui.horizontal(|ui| {
                    ui.label("Columns:");
                    if ui.button("All").clicked() { self.exp_sel.iter_mut().for_each(|x| *x = true); }
                    if ui.button("None").clicked() { self.exp_sel.iter_mut().for_each(|x| *x = false); }
                });
                egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible) // always show the bar, not only on hover
                    .show(ui, |ui| {
                    for (i, n) in names.iter().enumerate() { ui.checkbox(&mut self.exp_sel[i], n); }
                });
                ui.label("Exports every row of the current result (search, sort or SQL). A large SQL result is run again and streamed to the file.");
                if ui.button("Export…").clicked() { go = true; }
            });
            self.exp_open = open;
            if go { self.do_export(ctx); }
        }
        if self.assoc_open {
            let mut open = true;
            egui::Window::new("Open Parquet files with this app").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label("Adds Parquet Viewer to the \"Open with\" list for .parquet and .pq files, then opens the Windows\nDefault apps settings. Windows asks you to choose the default yourself.");
                ui.horizontal(|ui| {
                    if ui.button("Add and open Windows settings").clicked() {
                        self.assoc_msg = match crate::ui::assoc::register() {
                            Ok(()) => { crate::ui::assoc::open_default_apps_settings(); "Added. In the Windows settings, choose Parquet Viewer for .parquet and .pq.".to_string() }
                            Err(e) => format!("Could not add: {e}"),
                        };
                    }
                    if ui.button("Remove").clicked() {
                        self.assoc_msg = match crate::ui::assoc::unregister() { Ok(()) => "Removed from the Open with list.".to_string(), Err(e) => format!("Could not remove: {e}") };
                    }
                });
                if !self.assoc_msg.is_empty() { ui.label(self.assoc_msg.clone()); }
                ui.weak("If you move this program to another folder, add it again.");
            });
            self.assoc_open = open;
        }
        if self.schema_open {
            if self.schema_info.is_none() { self.schema_info = self.t.as_ref().map(|t| schema_info(t)); }
            let mut open = true;
            egui::Window::new("Schema").open(&mut open).default_width(920.0).default_height(520.0).resizable(true).show(ctx, |ui| {
                match &self.schema_info {
                    None => { ui.label("Open a file or folder first."); }
                    Some(si) => {
                        ui.label(format!("{} file(s)  |  {} rows  |  {} row groups  |  {} columns  |  {:.1} MB on disk", si.files, si.rows, si.rgs, si.cols.len(), si.bytes as f64 / 1e6));
                        ui.label(format!("Rows per row group: smallest {}, average {}, largest {}", si.rg_rows.0, si.rg_rows.1, si.rg_rows.2));
                        ui.label(format!("Written by: {}  (Parquet format version {})", si.created_by, si.version));
                        ui.separator();
                        egui::ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| {
                            TableBuilder::new(ui).striped(true).resizable(true).max_scroll_height(420.0)
                                .column(Column::initial(170.0).at_least(60.0).clip(true))
                                .column(Column::initial(120.0).at_least(50.0).clip(true))
                                .column(Column::initial(110.0).at_least(50.0).clip(true))
                                .column(Column::initial(70.0).at_least(40.0).clip(true))
                                .column(Column::initial(100.0).at_least(50.0).clip(true))
                                .column(Column::initial(200.0).at_least(60.0).clip(true))
                                .column(Column::initial(150.0).at_least(50.0).clip(true))
                                .column(Column::initial(150.0).at_least(50.0).clip(true))
                                .header(22.0, |mut h| { for t in ["Column", "Type", "Parquet type", "Nullable", "Compression", "Encodings", "Min", "Max"] { h.col(|ui| { ui.strong(t); }); } })
                                .body(|body| body.rows(20.0, si.cols.len(), |mut row| {
                                    let c = &si.cols[row.index()];
                                    for v in [&c.name, &c.arrow_type, &c.physical, &c.nullable, &c.compression, &c.encodings, &c.min, &c.max] {
                                        row.col(|ui| { cell_label(ui, v); });
                                    }
                                }));
                        });
                        ui.weak("Compression, encodings and the file details come from the first row group / first file. Min and Max cover all row groups, for numbers and text only.");
                    }
                }
            });
            self.schema_open = open;
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            self.table_rect = Some(ui.max_rect());
            if let Some(res) = &self.sql_view {
                let n = res.rows.len() as u64;
                let (top, vn) = (self.top, self.vis.min(n));
                let out = egui::ScrollArea::horizontal().auto_shrink([false, false]).horizontal_scroll_offset(self.hx).show(ui, |ui| {
                TableBuilder::new(ui).striped(true).resizable(true).vscroll(false)
                    .columns(Column::initial(140.0).at_least(40.0).clip(true), res.cols.len().max(1))
                    .header(22.0, |mut h| { for c in &res.cols { h.col(|ui| { ui.strong(c); }); } })
                    .body(|body| body.rows(20.0, vn as usize, |mut row| {
                        if let Some(r) = res.rows.get((top + row.index() as u64) as usize) { for c in r { row.col(|ui| { cell_label(ui, c); }); } }
                    }));
                });
                self.hx = out.state.offset.x;
                return;
            }
            let Some(t) = self.t.clone() else { return };
            let n = self.count();
            let vn = self.vis.min(n);
            let top = self.top.min(n.saturating_sub(vn)); // vis may have grown since `top` was clamped
            let mut need: Vec<(usize, u64)> = vec![];
            let (cache, view) = (&self.cache, &self.view);
            let ss = self.sort_state;
            let mut clicked: Option<usize> = None;
            let out = egui::ScrollArea::horizontal().auto_shrink([false, false]).horizontal_scroll_offset(self.hx).show(ui, |ui| {
            TableBuilder::new(ui).striped(true).resizable(true).vscroll(false)
                .columns(Column::initial(140.0).at_least(40.0).clip(true), t.names.len())
                .header(22.0, |mut h| {
                    for (ci, n) in t.names.iter().enumerate() {
                        h.col(|ui| {
                            let mark = match ss { Some((c, d)) if c == ci => if d { " v" } else { " ^" }, _ => "" };
                            let b = egui::Button::new(egui::RichText::new(format!("{n}{mark}")).strong()).frame(false);
                            if ui.add(b).on_hover_text("Click to sort: top 1000 (click again to reverse)").clicked() { clicked = Some(ci); }
                        });
                    }
                })
                .body(|body| body.rows(20.0, vn as usize, |mut row| {
                    let i = top + row.index() as u64;
                    if i >= n { return; }
                    let id = match view.as_ref() { Some(v) => match v.get(i as usize) { Some(&x) => x, None => return }, None => i };
                    let (g, local) = t.locate(id);
                    let key = (g, local / BLOCK);
                    match cache.get(&key).and_then(|b| b.get((local % BLOCK) as usize)) {
                        Some(r) => for c in r { row.col(|ui| { cell_label(ui, c); }); },
                        None => { need.push(key); for _ in 0..t.names.len() { row.col(|ui| { ui.weak("…"); }); } }
                    }
                }));
            });
            self.hx = out.state.offset.x;
            for k in need { self.request(k, ctx, 6); }
            // prefetch a block ahead and behind, so ordinary scrolling rarely shows placeholders
            let ahead: Vec<(usize, u64)> = [top + vn + BLOCK / 2, top + vn + BLOCK, top.saturating_sub(BLOCK / 2)].iter().filter(|&&r| r < n).filter_map(|&r| {
                let id = match self.view.as_ref() { Some(v) => *v.get(r as usize)?, None => r };
                let (g, l) = t.locate(id);
                Some((g, l / BLOCK))
            }).collect();
            for k in ahead { self.request(k, ctx, 3); }
            if let Some(ci) = clicked {
                self.col = Some(ci);
                let desc = matches!(self.sort_state, Some((c, false)) if c == ci);
                self.sort(desc, ctx);
            }
        });
    }
}

pub(crate) fn run() -> eframe::Result {
    let mut app = App::default();
    if let Some(p) = std::env::args().nth(1) { app.open_path(p.into()); }
    let icon = egui::IconData { rgba: include_bytes!("../../assets/icon_128.rgba").to_vec(), width: 128, height: 128 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_icon(icon).with_inner_size([1280.0, 760.0]).with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native("Parquet Viewer", options, Box::new(|_| Ok(Box::new(app))))
}

/// A table cell: right-click -> Copy.
fn cell_label(ui: &mut egui::Ui, text: &str) {
    let r = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
    r.context_menu(|ui| {
        if ui.button("Copy").clicked() {
            ui.output_mut(|o| o.copied_text = text.to_string());
            ui.close_menu();
        }
    });
}
