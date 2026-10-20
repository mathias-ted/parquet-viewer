//! Row-group skipping: min/max stats, bloom filters, numeric range queries.
use crate::prelude::*;

/// True if min/max stats prove `q` cannot appear in column `c` of row group `g` (equals only).
pub(crate) fn prunable(t: &Table, g: usize, c: usize, q: &str) -> bool {
    // unsigned logical types are stored as signed physical ints; their stats order differently
    if matches!(t.schema().field(c).data_type(), DataType::UInt8 | DataType::UInt16 | DataType::UInt32 | DataType::UInt64) { return false; }
    match t.rg_meta(g).column(c).statistics() {
        Some(Statistics::Int32(s)) => match (q.parse::<i64>(), s.min_opt(), s.max_opt()) {
            (Ok(x), Some(a), Some(b)) => x < *a as i64 || x > *b as i64,
            _ => false,
        },
        Some(Statistics::Int64(s)) => match (q.parse::<i64>(), s.min_opt(), s.max_opt()) {
            (Ok(x), Some(a), Some(b)) => x < *a || x > *b,
            _ => false,
        },
        Some(Statistics::Double(s)) => match (q.parse::<f64>(), s.min_opt(), s.max_opt()) {
            (Ok(x), Some(a), Some(b)) => x < *a || x > *b,
            _ => false,
        },
        Some(Statistics::ByteArray(s)) => match (s.min_opt().and_then(|v| v.as_utf8().ok()), s.max_opt().and_then(|v| v.as_utf8().ok())) {
            (Some(a), Some(b)) => q < a || q > b,
            _ => false,
        },
        _ => false,
    }
}

// ---------- numeric range search: ">5", ">=5", "<5", "<=5", "5..10" ----------
#[derive(Clone, Copy)]
pub(crate) struct Range { pub(crate) lo: f64, pub(crate) lo_inc: bool, pub(crate) hi: f64, pub(crate) hi_inc: bool }

impl Range {
    pub(crate) fn contains(&self, x: f64) -> bool { (x > self.lo || (self.lo_inc && x == self.lo)) && (x < self.hi || (self.hi_inc && x == self.hi)) }
}

pub(crate) fn parse_range(q: &str) -> Option<Range> {
    let q = q.trim();
    let n = |s: &str| s.trim().parse::<f64>().ok().filter(|x| !x.is_nan());
    let (inf, ninf) = (f64::INFINITY, f64::NEG_INFINITY);
    if let Some(r) = q.strip_prefix(">=") { return Some(Range { lo: n(r)?, lo_inc: true, hi: inf, hi_inc: true }); }
    if let Some(r) = q.strip_prefix('>') { return Some(Range { lo: n(r)?, lo_inc: false, hi: inf, hi_inc: true }); }
    if let Some(r) = q.strip_prefix("<=") { return Some(Range { lo: ninf, lo_inc: true, hi: n(r)?, hi_inc: true }); }
    if let Some(r) = q.strip_prefix('<') { return Some(Range { lo: ninf, lo_inc: true, hi: n(r)?, hi_inc: false }); }
    let (a, b) = q.split_once("..")?;
    Some(Range { lo: n(a)?, lo_inc: true, hi: n(b)?, hi_inc: true })
}

pub(crate) fn is_numeric(dt: &DataType) -> bool {
    use DataType::*;
    matches!(dt, Int8 | Int16 | Int32 | Int64 | UInt8 | UInt16 | UInt32 | UInt64 | Float32 | Float64)
}

/// True if min/max stats prove no value of column `c` in row group `g` lies in `r`.
pub(crate) fn range_prunable(t: &Table, g: usize, c: usize, r: &Range) -> bool {
    use DataType::*;
    if !matches!(t.schema().field(c).data_type(), Int8 | Int16 | Int32 | Int64 | Float32 | Float64) { return false; } // unsigned: stats order differs
    let (mn, mx) = match t.rg_meta(g).column(c).statistics() {
        Some(Statistics::Int32(s)) => (s.min_opt().map(|v| *v as f64), s.max_opt().map(|v| *v as f64)),
        Some(Statistics::Int64(s)) => (s.min_opt().map(|v| *v as f64), s.max_opt().map(|v| *v as f64)),
        Some(Statistics::Float(s)) => (s.min_opt().map(|v| *v as f64), s.max_opt().map(|v| *v as f64)),
        Some(Statistics::Double(s)) => (s.min_opt().copied(), s.max_opt().copied()),
        _ => return false,
    };
    let (Some(mn), Some(mx)) = (mn, mx) else { return false };
    mx < r.lo || (mx == r.lo && !r.lo_inc) || mn > r.hi || (mn == r.hi && !r.hi_inc)
}

pub(crate) fn match_range(b: &arrow::record_batch::RecordBatch, r: &Range) -> Vec<usize> {
    let Ok(a) = cast(b.column(0), &DataType::Float64) else { return vec![] };
    let a = a.as_primitive::<Float64Type>();
    (0..a.len()).filter(|&i| a.is_valid(i) && r.contains(a.value(i))).collect()
}

/// True if the column chunk's bloom filter proves `q` is absent (no false negatives).
/// Only touches the file when the chunk actually has a bloom filter.
pub(crate) fn bloom_absent(t: &Table, g: usize, c: usize, q: &str) -> bool {
    let col = t.rg_meta(g).column(c);
    if col.bloom_filter_offset().is_none() { return false; }
    let run = || -> Res<bool> {
        let (f, l) = t.rg_file[g];
        let r = SerializedFileReader::new(File::open(&t.files[f].path)?)?;
        let rg = r.get_row_group(l)?;
        let Some(bf) = rg.get_column_bloom_filter(c) else { return Ok(false) };
        Ok(match col.column_type() {
            PType::INT32 => q.parse::<i32>().map(|x| !bf.check(&x)).unwrap_or(false),
            PType::INT64 => q.parse::<i64>().map(|x| !bf.check(&x)).unwrap_or(false),
            PType::BYTE_ARRAY => !bf.check(&ByteArray::from(q)),
            _ => false,
        })
    };
    run().unwrap_or(false)
}
