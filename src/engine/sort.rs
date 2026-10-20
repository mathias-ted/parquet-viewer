//! Top-k sorting via per-batch partial sorts and merging. Can be restricted to a set of row ids
//! (e.g. the current search results) so only those rows are sorted.
use crate::prelude::*;

pub(crate) type Cand = (ArrayRef, Vec<u64>); // candidate values + their global row ids

pub(crate) fn local_top(vals: ArrayRef, ids: Vec<u64>, k: usize, desc: bool) -> Res<Cand> {
    let idx = sort_to_indices(&vals, Some(SortOptions { descending: desc, nulls_first: false }), Some(k))?;
    let v = take(&vals, &idx, None)?;
    let ids = idx.values().iter().map(|&i| ids[i as usize]).collect();
    Ok((v, ids))
}

pub(crate) fn merge_top(a: Cand, b: Cand, k: usize, desc: bool) -> Res<Cand> {
    let v = concat(&[a.0.as_ref(), b.0.as_ref()])?;
    let mut ids = a.1;
    ids.extend(b.1);
    local_top(v, ids, k, desc)
}

/// `subset` must be ascending. Row groups and batches with no subset rows are skipped without decoding.
pub(crate) fn topk_rg(t: &Table, g: usize, col: usize, k: usize, desc: bool, subset: Option<&[u64]>) -> Res<Option<Cand>> {
    use arrow::array::UInt32Array;
    let in_rg = subset.map(|s| {
        let a = s.partition_point(|&x| x < t.starts[g]);
        let b = s.partition_point(|&x| x < t.starts[g + 1]);
        &s[a..b]
    });
    if in_rg.map_or(false, |s| s.is_empty()) { return Ok(None); }
    let b = t.builder(g, None)?;
    let mask = ProjectionMask::roots(b.parquet_schema(), [col]);
    let mut off = t.starts[g];
    let mut best: Option<Cand> = None;
    for batch in b.with_projection(mask).with_batch_size(16384).build()? {
        let batch = batch?;
        let n = batch.num_rows() as u64;
        let part = match in_rg {
            None => local_top(batch.column(0).clone(), (off..off + n).collect(), k, desc)?,
            Some(sl) => {
                let a = sl.partition_point(|&x| x < off);
                let e = sl.partition_point(|&x| x < off + n);
                if a == e { off += n; continue; }
                let idx = UInt32Array::from_iter_values(sl[a..e].iter().map(|&x| (x - off) as u32));
                let vals = take(batch.column(0).as_ref(), &idx, None)?;
                local_top(vals, sl[a..e].to_vec(), k, desc)?
            }
        };
        off += n;
        best = Some(match best { None => part, Some(a) => merge_top(a, part, k, desc)? });
    }
    Ok(best)
}

/// A column statistic in a comparable form (ints, dates and timestamps as i64).
#[derive(Clone, PartialEq, PartialOrd, Debug)]
pub(crate) enum Key { I(i64), F(f64), S(Vec<u8>) }

/// Column types whose min/max statistics we can compare safely (unsigned ints are excluded on purpose).
fn sortable(dt: &DataType) -> bool {
    use DataType::*;
    matches!(dt, Int8 | Int16 | Int32 | Int64 | Float32 | Float64 | Utf8 | Date32 | Timestamp(_, _))
}

/// (min, max) the file recorded for one column chunk, if any.
pub(crate) fn rg_range(t: &Table, g: usize, col: usize) -> Option<(Key, Key)> {
    match t.rg_meta(g).column(col).statistics()? {
        Statistics::Int32(s) => Some((Key::I(*s.min_opt()? as i64), Key::I(*s.max_opt()? as i64))),
        Statistics::Int64(s) => Some((Key::I(*s.min_opt()?), Key::I(*s.max_opt()?))),
        Statistics::Float(s) => Some((Key::F(*s.min_opt()? as f64), Key::F(*s.max_opt()? as f64))),
        Statistics::Double(s) => Some((Key::F(*s.min_opt()?), Key::F(*s.max_opt()?))),
        Statistics::ByteArray(s) => Some((Key::S(s.min_opt()?.data().to_vec()), Key::S(s.max_opt()?.data().to_vec()))),
        _ => None,
    }
}

/// The k-th best value found so far (`vals` is sorted best-first). None until k real values exist.
pub(crate) fn kth_key(vals: &ArrayRef, k: usize) -> Option<Key> {
    if k == 0 || vals.len() < k { return None; }
    let one = vals.slice(k - 1, 1);
    match vals.data_type() {
        DataType::Float32 | DataType::Float64 => {
            let a = cast(&one, &DataType::Float64).ok()?;
            let f = a.as_primitive::<Float64Type>();
            if f.is_null(0) { None } else { Some(Key::F(f.value(0))) }
        }
        DataType::Utf8 => {
            let s = one.as_string::<i32>();
            if s.is_null(0) { None } else { Some(Key::S(s.value(0).as_bytes().to_vec())) }
        }
        _ => {
            let a = cast(&one, &DataType::Int64).ok()?;
            let i = a.as_primitive::<arrow::datatypes::Int64Type>();
            if i.is_null(0) { None } else { Some(Key::I(i.value(0))) }
        }
    }
}

/// Top-k of one column. Row groups sit in a queue ordered best-first (by their min, or max when descending);
/// every worker takes the next one and first checks the current k-th best value. If the row group's range
/// cannot beat it, nothing later in the queue can either, so everyone stops. Extra reading is at most about
/// one row group per thread. If the file has no usable statistics, or the data is mixed so ranges overlap,
/// nothing is skipped and the result is the same as a full scan.
pub(crate) fn build_topk(t: &Table, col: usize, k: usize, desc: bool, subset: Option<&[u64]>, job: &Job) -> Res<Vec<u64>> {
    let sorted: Option<Vec<u64>> = subset.map(|s| { let mut v = s.to_vec(); v.sort_unstable(); v });
    let sub = sorted.as_deref();
    let n = t.num_rgs();
    let usable = t.flat && sortable(t.schema().field(col).data_type());
    let mut order: Vec<(usize, Option<(Key, Key)>)> = (0..n).map(|g| (g, if usable { rg_range(t, g, col) } else { None })).collect();
    if usable {
        use std::cmp::Ordering::*;
        order.sort_by(|a, b| match (&a.1, &b.1) {
            (None, None) => Equal,
            (None, _) => Less, // unknown range: must be read, so read first
            (_, None) => Greater,
            (Some(x), Some(y)) => { let (p, q) = if desc { (&y.1, &x.1) } else { (&x.0, &y.0) }; p.partial_cmp(q).unwrap_or(Equal) }
        });
    }
    let next = AtomicUsize::new(0);   // next queue position to hand out
    let read = AtomicUsize::new(0);   // row groups actually processed
    let stop = AtomicBool::new(false);
    // (best candidates so far, their k-th value, first error)
    let state = std::sync::Mutex::new((None::<Cand>, None::<Key>, None::<String>));
    (0..rayon::current_num_threads().max(1)).into_par_iter().for_each(|_| loop {
        if stop.load(Relaxed) { break; }
        if job.cancelled() { stop.store(true, Relaxed); break; }
        let i = next.fetch_add(1, Relaxed);
        if i >= n { break; }
        let (g, range) = (order[i].0, &order[i].1);
        let skip = {
            let st = state.lock().unwrap();
            match (&st.1, range) {
                (Some(th), Some((mn, mx))) => if desc { mx < th } else { mn > th }, // strictly worse than the k-th best
                _ => false,
            }
        };
        if skip { stop.store(true, Relaxed); break; } // sorted queue: every later row group is skippable too
        match topk_rg(t, g, col, k, desc, sub) {
            Ok(part) => {
                read.fetch_add(1, Relaxed);
                if let Some(p) = part {
                    let mut st = state.lock().unwrap();
                    let merged = match st.0.take() { None => Ok(p), Some(a) => merge_top(a, p, k, desc) };
                    match merged {
                        Ok(m) => { st.1 = if usable { kth_key(&m.0, k) } else { None }; st.0 = Some(m); }
                        Err(e) => { st.2 = Some(e.to_string()); stop.store(true, Relaxed); }
                    }
                }
                job.tick();
            }
            Err(e) => { state.lock().unwrap().2 = Some(e.to_string()); stop.store(true, Relaxed); }
        }
    });
    if job.cancelled() { return Err("cancelled".into()); }
    let (best, _, err) = state.into_inner().unwrap();
    if let Some(e) = err { return Err(e.into()); }
    let skipped = n - read.load(Relaxed); // row groups never read
    job.skipped.store(skipped, Relaxed);
    job.add(skipped); // fill the rest of the progress bar
    Ok(best.map(|a| a.1).unwrap_or_default())
}
