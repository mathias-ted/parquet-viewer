//! File and column information for the Schema window. Everything comes from the file footers
//! that are already loaded, so building it reads nothing from the data itself.
use crate::prelude::*;

pub(crate) struct ColInfo {
    pub(crate) name: String,
    pub(crate) arrow_type: String,   // logical type, e.g. Utf8, Int64, Timestamp(...)
    pub(crate) physical: String,     // Parquet storage type, e.g. BYTE_ARRAY
    pub(crate) nullable: String,
    pub(crate) compression: String,  // first row group
    pub(crate) encodings: String,    // first row group
    pub(crate) min: String,          // smallest value over all row groups (numbers and text only)
    pub(crate) max: String,
}

pub(crate) struct SchemaInfo {
    pub(crate) files: usize,
    pub(crate) rows: u64,
    pub(crate) rgs: usize,
    pub(crate) bytes: u64,
    pub(crate) created_by: String,   // first file
    pub(crate) version: i32,         // Parquet format version, first file
    pub(crate) rg_rows: (u64, u64, u64), // smallest / average / largest row group, in rows
    pub(crate) cols: Vec<ColInfo>,
}

/// "ZSTD(ZstdLevel(1))" -> "ZSTD"
pub(crate) fn short(s: String) -> String {
    match s.find('(') { Some(i) => s[..i].to_string(), None => s }
}

fn key_text(k: &Key) -> String {
    match k {
        Key::I(v) => v.to_string(),
        Key::F(v) => format!("{v}"),
        Key::S(b) => {
            let s = String::from_utf8_lossy(b);
            if s.chars().count() > 40 { format!("{}...", s.chars().take(40).collect::<String>()) } else { s.to_string() }
        }
    }
}

/// Min/max are shown only where the stored number or text means what it says (not dates, not unsigned ints).
fn shows_range(dt: &DataType) -> bool {
    use DataType::*;
    matches!(dt, Int8 | Int16 | Int32 | Int64 | Float32 | Float64 | Utf8)
}

pub(crate) fn schema_info(t: &Table) -> SchemaInfo {
    let rows: Vec<u64> = (0..t.num_rgs()).map(|g| t.starts[g + 1] - t.starts[g]).collect();
    let rg_rows = (
        rows.iter().copied().min().unwrap_or(0),
        if rows.is_empty() { 0 } else { t.total() / rows.len() as u64 },
        rows.iter().copied().max().unwrap_or(0),
    );
    let bytes = t.files.iter().map(|f| std::fs::metadata(&f.path).map_or(0, |m| m.len())).sum();
    let fm = t.files[0].arm.metadata().file_metadata();
    let cols = t.schema().fields().iter().enumerate().map(|(i, f)| {
        let (mut physical, mut compression, mut encodings, mut min, mut max) =
            ("-".to_string(), "-".to_string(), "-".to_string(), "-".to_string(), "-".to_string());
        if t.flat && t.num_rgs() > 0 {
            let c = t.rg_meta(0).column(i);
            physical = format!("{:?}", c.column_type());
            compression = short(format!("{:?}", c.compression()));
            let mut enc: Vec<String> = c.encodings().iter().map(|e| format!("{e:?}")).collect();
            enc.sort();
            enc.dedup();
            encodings = enc.join(", ");
            if shows_range(f.data_type()) {
                let (mut lo, mut hi, mut complete): (Option<Key>, Option<Key>, bool) = (None, None, true);
                for g in 0..t.num_rgs() {
                    match rg_range(t, g, i) {
                        Some((a, b)) => {
                            if lo.as_ref().map_or(true, |l| a < *l) { lo = Some(a); }
                            if hi.as_ref().map_or(true, |h| b > *h) { hi = Some(b); }
                        }
                        None => { complete = false; break; } // some row group has no statistics: no honest overall range
                    }
                }
                if complete {
                    if let (Some(l), Some(h)) = (&lo, &hi) { min = key_text(l); max = key_text(h); }
                }
            }
        }
        ColInfo {
            name: f.name().clone(),
            arrow_type: f.data_type().to_string(),
            physical,
            nullable: if f.is_nullable() { "yes" } else { "no" }.to_string(),
            compression,
            encodings,
            min,
            max,
        }
    }).collect();
    SchemaInfo {
        files: t.files.len(),
        rows: t.total(),
        rgs: t.num_rgs(),
        bytes,
        created_by: fm.created_by().unwrap_or("unknown").to_string(),
        version: fm.version(),
        rg_rows,
        cols,
    }
}
