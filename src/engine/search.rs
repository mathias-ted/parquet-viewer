//! Parallel projected scans, dictionary fast path, SIMD substring match.
use crate::prelude::*;

/// True if every data page of this column chunk is dictionary-encoded (no plain fallback).
/// Unknown (no encoding stats) counts as false, so the plain path is used.
pub(crate) fn pure_dict(c: &ColumnChunkMetaData) -> bool {
    c.dictionary_page_offset().is_some()
        && c.page_encoding_stats().map_or(false, |v| {
            v.iter()
                .filter(|e| matches!(e.page_type, PageType::DATA_PAGE | PageType::DATA_PAGE_V2))
                .all(|e| matches!(e.encoding, Encoding::PLAIN_DICTIONARY | Encoding::RLE_DICTIONARY))
        })
}

/// Reader metadata that decodes dictionary-encoded Utf8 columns as Dictionary<Int32, Utf8>
/// (no string expansion). Only for searched columns where most row groups have a dictionary page.
pub(crate) fn dict_meta(t: &Table, cols: &[usize]) -> Option<Vec<ArrowReaderMetadata>> {
    let n = t.num_rgs();
    if !t.flat || n == 0 || std::env::var_os("PQV_NODICT").is_some() { return None; } // debug switch: PQV_NODICT=1 turns the dictionary read hint off
    let schema = t.schema();
    let use_dict: Vec<bool> = schema.fields().iter().enumerate().map(|(i, f)| {
        let dict_rgs = (0..n).filter(|&g| pure_dict(t.rg_meta(g).column(i))).count();
        cols.contains(&i) && *f.data_type() == DataType::Utf8 && dict_rgs * 2 >= n
    }).collect();
    if !use_dict.iter().any(|&d| d) { return None; }
    let fields: Vec<Arc<Field>> = schema.fields().iter().enumerate().map(|(i, f)| {
        if use_dict[i] { Arc::new(Field::new(f.name(), DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)), f.is_nullable())) } else { f.clone() }
    }).collect();
    let new_schema = Arc::new(Schema::new(fields));
    // one adjusted metadata per file (same schema override, each file's own footer)
    t.files.iter().map(|f| ArrowReaderMetadata::try_new(f.arm.metadata().clone(), ArrowReaderOptions::new().with_schema(new_schema.clone())).ok()).collect()
}

/// Evaluate the predicate once per dictionary (cached across batches), then gather per-row results
/// via the integer keys. The slot keeps the dictionary array alive, so a matching buffer address
/// really is the same buffer (a freed buffer could otherwise be reallocated at the same address).
pub(crate) type DictSlot = Option<(ArrayRef, BooleanArray)>;

pub(crate) fn same_dict(v: &ArrayRef, sa: &StringArray) -> bool {
    v.as_string_opt::<i32>().map_or(false, |c| {
        c.len() == sa.len() && c.value_data().as_ptr() == sa.value_data().as_ptr() && c.value_offsets().as_ptr() == sa.value_offsets().as_ptr()
    })
}

pub(crate) fn dict_match(col: &ArrayRef, pat: &Scalar<StringArray>, equals: bool, ac: Option<&AhoCorasick>, slot: &mut DictSlot) -> Option<BooleanArray> {
    let d = col.as_dictionary_opt::<Int32Type>()?;
    let vals = d.values();
    let sa = vals.as_string_opt::<i32>()?;
    let cached = slot.as_ref().filter(|t| same_dict(&t.0, sa)).map(|t| t.1.clone());
    let vm = match cached {
        Some(m) => m,
        None => {
            let m = match ac {
                Some(ac) => fast_contains(sa, ac),
                None => (if equals { eq(&sa, pat) } else { ilike(&sa, pat) }).ok()?,
            };
            *slot = Some((vals.clone(), m.clone()));
            m
        }
    };
    let r = take(&vm, d.keys(), None).ok()?;
    r.as_boolean_opt().cloned()
}

/// Fast ASCII case-insensitive substring match over a plain Utf8 column: scan the contiguous
/// value buffer with a SIMD-accelerated searcher and jump to the next row after each match.
pub(crate) fn fast_contains(sa: &StringArray, ac: &AhoCorasick) -> BooleanArray {
    let offs = sa.value_offsets();
    let n = sa.len();
    let mut hit = vec![false; n];
    if n > 0 {
        let data = sa.value_data();
        let (base, end) = (offs[0] as usize, offs[n] as usize);
        let (mut p, mut row) = (base, 0usize);
        while p < end {
            let Some(m) = ac.find(&data[p..end]) else { break };
            let (s, e) = (p + m.start(), p + m.end());
            while (offs[row + 1] as usize) <= s { row += 1; }
            let row_end = offs[row + 1] as usize;
            if e <= row_end { hit[row] = true; } // else: match straddles two rows, ignore it
            p = row_end; // next search starts at the following row
        }
    }
    BooleanArray::from(hit)
}

pub(crate) fn esc(q: &str) -> String { q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_") }

/// Vectorized match over every projected column of a batch; returns matching row indices.
pub(crate) fn match_batch(b: &arrow::record_batch::RecordBatch, q: &str, equals: bool, dc: &mut Vec<DictSlot>) -> Vec<usize> {
    if dc.len() < b.num_columns() { dc.resize(b.num_columns(), None); }
    let pat: Scalar<StringArray> = if equals { StringArray::new_scalar(q) } else { StringArray::new_scalar(format!("%{}%", esc(q))) };
    let ac = if !equals && q.is_ascii() && !q.is_empty() { AhoCorasick::builder().ascii_case_insensitive(true).build([q]).ok() } else { None };
    let mut acc: Option<BooleanArray> = None;
    for (ci, col) in b.columns().iter().enumerate() {
        let m = if matches!(col.data_type(), DataType::Dictionary(_, _)) {
            match dict_match(col, &pat, equals, ac.as_ref(), &mut dc[ci]) { Some(m) => m, None => continue }
        } else if let (Some(ac), Some(sa)) = (&ac, col.as_string_opt::<i32>()) {
            fast_contains(sa, ac)
        } else {
            let Ok(s) = cast(col, &DataType::Utf8) else { continue };
            match if equals { eq(&s, &pat) } else { ilike(&s, &pat) } { Ok(m) => m, Err(_) => continue }
        };
        acc = Some(match acc { None => m, Some(a) => or_kleene(&a, &m).unwrap_or(a) });
    }
    acc.map(|m| (0..m.len()).filter(|&i| m.is_valid(i) && m.value(i)).collect()).unwrap_or_default()
}

pub(crate) enum Msg { Hits(Vec<u64>), Done }

pub(crate) struct Scan { pub(crate) rx: Receiver<Msg>, pub(crate) cancel: Arc<AtomicBool>, pub(crate) scanned: Arc<AtomicUsize>, pub(crate) skipped: Arc<AtomicUsize>, pub(crate) t0: Instant, pub(crate) done: Option<f64>, pub(crate) dec: Arc<AtomicU64>, pub(crate) mat: Arc<AtomicU64> }

pub(crate) fn scan_rg(t: &Table, arm: &[ArrowReaderMetadata], g: usize, cols: &[usize], q: &str, eq_: bool, range: Option<Range>, cancel: &AtomicBool, dec: &AtomicU64, mat: &AtomicU64, tx: &Sender<Msg>) -> Res<()> {
    let b = t.builder(g, Some(arm))?;
    let mask = ProjectionMask::roots(b.parquet_schema(), cols.iter().copied()); // read only searched columns
    let mut reader = b.with_projection(mask).with_batch_size(16384).build()?;
    let mut off = t.starts[g];
    let mut dc: Vec<DictSlot> = Vec::new();
    loop {
        if cancel.load(Relaxed) { break; }
        let t0 = Instant::now();
        let next = reader.next(); // parquet decode happens here
        dec.fetch_add(t0.elapsed().as_nanos() as u64, Relaxed);
        let Some(batch) = next else { break };
        let batch = batch?;
        let t1 = Instant::now();
        let idxs = match &range { Some(r) => match_range(&batch, r), None => match_batch(&batch, q, eq_, &mut dc) };
        let hits: Vec<u64> = idxs.into_iter().map(|i| off + i as u64).collect();
        mat.fetch_add(t1.elapsed().as_nanos() as u64, Relaxed);
        off += batch.num_rows() as u64;
        if !hits.is_empty() { let _ = tx.send(Msg::Hits(hits)); }
    }
    Ok(())
}

pub(crate) fn start_scan(t: &Arc<Table>, cols: Vec<usize>, q: String, equals: bool, range: Option<Range>, idx: Option<Arc<TriIdx>>, ctx: egui::Context) -> Scan {
    let (tx, rx) = channel();
    let (cancel, scanned, skipped) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (t, c2, s2, k2) = (t.clone(), cancel.clone(), scanned.clone(), skipped.clone());
    let dm = dict_meta(&t, &cols).unwrap_or_else(|| t.files.iter().map(|f| f.arm.clone()).collect());
    let (dec, mat) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let (d2, m2) = (dec.clone(), mat.clone());
    std::thread::spawn(move || {
        let n = t.num_rgs();
        (0..n).into_par_iter().for_each_with(tx.clone(), |tx, g| {
            if c2.load(Relaxed) { return; }
            // trigram index: drop columns whose fragments rule out the query in this row group
            let rg_cols: Vec<usize> = match &idx {
                Some(ix) if !equals && q.is_ascii() && q.len() >= 3 => cols.iter().copied()
                    .filter(|&c| ix.cols.iter().position(|&x| x == c).map_or(true, |ci| may_contain(ix.slice(g, ci), q.as_bytes()))).collect(),
                _ => cols.clone(),
            };
            if rg_cols.is_empty() || range.map_or(false, |r| t.flat && range_prunable(&t, g, cols[0], &r)) || (equals && t.flat && cols.iter().all(|&c| prunable(&t, g, c, &q) || bloom_absent(&t, g, c, &q))) { k2.fetch_add(1, Relaxed); return; }
            let _ = scan_rg(&t, &dm, g, &rg_cols, &q, equals, range, &c2, &d2, &m2, tx);
            s2.fetch_add(1, Relaxed);
            ctx.request_repaint();
        });
        let _ = tx.send(Msg::Done);
        ctx.request_repaint();
    });
    Scan { rx, cancel, scanned, skipped, t0: Instant::now(), done: None, dec, mat }
}
