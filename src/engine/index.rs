//! Sidecar indexes: trigram bloom (contains) and exact-value hash index (equals).
use crate::prelude::*;

// ---------- trigram index: per (row group, string column) bloom of lowercase 3-byte fragments ----------
pub(crate) const BYTES: usize = (1 << 18) / 8;

pub(crate) struct TriIdx { pub(crate) cols: Vec<usize>, pub(crate) rgs: Vec<Vec<u8>> }

impl TriIdx { pub(crate) fn slice(&self, g: usize, ci: usize) -> &[u8] { &self.rgs[g][ci * BYTES..(ci + 1) * BYTES] } }

pub(crate) fn tri(w: &[u8]) -> u32 { ((w[0].to_ascii_lowercase() as u32) << 16) | ((w[1].to_ascii_lowercase() as u32) << 8) | w[2].to_ascii_lowercase() as u32 }

pub(crate) fn hs(t: u32) -> [usize; 2] { [(t.wrapping_mul(0x9E3779B1) >> 14) as usize, (t.wrapping_mul(0x85EBCA6B).rotate_left(15) >> 14) as usize] }

pub(crate) fn add_tris(bits: &mut [u8], v: &[u8]) { for w in v.windows(3) { for h in hs(tri(w)) { bits[h >> 3] |= 1 << (h & 7); } } }

/// False only if some query trigram is definitely absent (no false negatives). Query must be ASCII.
pub(crate) fn may_contain(bits: &[u8], q: &[u8]) -> bool { q.windows(3).all(|w| hs(tri(w)).iter().all(|&h| bits[h >> 3] & (1 << (h & 7)) != 0)) }

pub(crate) fn add_col(bits: &mut [u8], col: &ArrayRef) {
    let sa = match col.as_dictionary_opt::<Int32Type>() { Some(d) => d.values().as_string_opt::<i32>(), None => col.as_string_opt::<i32>() };
    if let Some(sa) = sa { for v in sa.iter().flatten() { add_tris(bits, v.as_bytes()); } }
}

pub(crate) fn index_rg(t: &Table, arm: &[ArrowReaderMetadata], g: usize, cols: &[usize], out: &mut [u8]) -> Res<()> {
    let b = t.builder(g, Some(arm))?;
    let mask = ProjectionMask::roots(b.parquet_schema(), cols.iter().copied());
    for batch in b.with_projection(mask).with_batch_size(16384).build()? {
        let batch = batch?;
        for (ci, col) in batch.columns().iter().enumerate() { add_col(&mut out[ci * BYTES..(ci + 1) * BYTES], col); }
    }
    Ok(())
}

pub(crate) fn build_idx(t: &Table, job: &Job) -> Option<TriIdx> {
    let cols: Vec<usize> = if t.flat { t.schema().fields().iter().enumerate().filter(|(_, f)| *f.data_type() == DataType::Utf8).map(|(i, _)| i).collect() } else { vec![] };
    if cols.is_empty() { return Some(TriIdx { cols, rgs: vec![] }); }
    let dm = dict_meta(t, &cols).unwrap_or_else(|| t.files.iter().map(|f| f.arm.clone()).collect()); // dictionary columns: index only the dictionary values
    let rgs = (0..t.num_rgs()).into_par_iter().map(|g| {
        if job.cancelled() { return Vec::new(); }
        let mut out = vec![0u8; cols.len() * BYTES];
        if index_rg(t, &dm, g, &cols, &mut out).is_err() { out.fill(0xFF); } // on error: never skip
        job.tick();
        out
    }).collect();
    if job.cancelled() { return None; }
    Some(TriIdx { cols, rgs })
}

pub(crate) fn cache_path(t: &Table) -> PathBuf {
    // key = path, size and modified time of every file, so any change re-indexes
    let mut h: u64 = 0xcbf29ce484222325;
    let mut feed = |bytes: &[u8]| for &b in bytes { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); };
    for f in &t.files {
        let m = std::fs::metadata(&f.path).ok();
        let len = m.as_ref().map_or(0, |m| m.len());
        let mt = m.and_then(|m| m.modified().ok()).and_then(|x| x.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        feed(f.path.to_string_lossy().as_bytes());
        feed(&len.to_le_bytes());
        feed(&mt.to_le_bytes());
    }
    std::env::temp_dir().join(format!("pqv-{h:016x}.tgx"))
}
/// Free memory a text index over this table needs (32 KB per row group per text column, plus a margin).
pub(crate) fn text_index_mem_need(t: &Table) -> u64 {
    let text_cols = if t.flat { t.schema().fields().iter().filter(|f| *f.data_type() == DataType::Utf8).count() } else { 0 };
    (t.num_rgs() as u64).saturating_mul(text_cols as u64).saturating_mul(BYTES as u64) / 10 * 12
}

/// Written straight to disk, one row group at a time: no second copy of the index in memory.
pub(crate) fn save_idx(t: &Table, ix: &TriIdx) {
    let path = cache_path(t);
    let Ok(f) = File::create(&path) else { return };
    let mut w = BufWriter::with_capacity(1 << 20, f);
    let ok = (|| -> std::io::Result<()> {
        w.write_all(b"TGX1")?;
        w.write_all(&(ix.cols.len() as u32).to_le_bytes())?;
        for c in &ix.cols { w.write_all(&(*c as u32).to_le_bytes())?; }
        w.write_all(&(ix.rgs.len() as u32).to_le_bytes())?;
        for r in &ix.rgs { w.write_all(r)?; }
        w.flush()
    })();
    if ok.is_err() { drop(w); let _ = std::fs::remove_file(&path); } // never leave a partial index behind
}

/// Read one row group at a time. Returns None (index simply not used) if the file is stale, damaged,
/// or too big for the free memory, instead of risking running out of memory when a file is opened.
pub(crate) fn load_idx(t: &Table) -> Option<TriIdx> {
    let path = cache_path(t);
    let len = std::fs::metadata(&path).ok()?.len();
    if len > free_memory() / 10 * 8 { return None; }
    let mut f = BufReader::with_capacity(1 << 20, File::open(&path).ok()?);
    let mut b4 = [0u8; 4];
    f.read_exact(&mut b4).ok()?;
    if &b4 != b"TGX1" { return None; }
    f.read_exact(&mut b4).ok()?;
    let nc = u32::from_le_bytes(b4) as usize;
    if nc == 0 || nc > 10_000 { return None; }
    let mut cols = Vec::with_capacity(nc);
    for _ in 0..nc { f.read_exact(&mut b4).ok()?; cols.push(u32::from_le_bytes(b4) as usize); }
    f.read_exact(&mut b4).ok()?;
    let nrg = u32::from_le_bytes(b4) as usize;
    if nrg != t.num_rgs() || len != (12 + 4 * nc) as u64 + nrg as u64 * nc as u64 * BYTES as u64 { return None; }
    let mut rgs = Vec::with_capacity(nrg);
    for _ in 0..nrg {
        let mut v = vec![0u8; nc * BYTES];
        f.read_exact(&mut v).ok()?;
        rgs.push(v);
    }
    Some(TriIdx { cols, rgs })
}

// ---------- exact-value index: sorted (hash, row) pairs for one column; serves "equals" in ms ----------
// Memory: 16 bytes per row while building and while loaded (100M rows = 1.6 GB). On disk: 12 bytes per row.
pub(crate) struct ExactIdx { pub(crate) pairs: Vec<(u64, u32)> } // sorted by hash, then row

pub(crate) fn h64(b: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &x in b { h ^= x as u64; h = h.wrapping_mul(0x100000001b3); }
    h ^= h >> 32; h = h.wrapping_mul(0x9E3779B97F4A7C15); h ^ (h >> 29)
}

pub(crate) fn exact_lookup(ix: &ExactIdx, q: &str) -> Vec<u64> {
    let h = h64(q.as_bytes());
    let mut i = ix.pairs.partition_point(|p| p.0 < h);
    let mut out = vec![];
    while i < ix.pairs.len() && ix.pairs[i].0 == h { out.push(ix.pairs[i].1 as u64); i += 1; }
    out
}

/// Bytes of free memory an exact index over this table needs (with a safety margin).
pub(crate) fn exact_mem_need(t: &Table) -> u64 { t.total().saturating_mul(16) / 100 * 115 }
pub(crate) fn free_memory() -> u64 {
    let mut s = sysinfo::System::new();
    s.refresh_memory();
    s.available_memory()
}

pub(crate) fn exact_rg(t: &Table, g: usize, col: usize) -> Res<Vec<(u64, u32)>> {
    let b = t.builder(g, None)?;
    let mask = ProjectionMask::roots(b.parquet_schema(), [col]);
    let mut off = t.starts[g];
    let mut v = vec![];
    for batch in b.with_projection(mask).with_batch_size(16384).build()? {
        let batch = batch?;
        let a = cast(batch.column(0), &DataType::Utf8)?; // same text form the search compares against
        let sa = a.as_string::<i32>();
        for i in 0..sa.len() { if sa.is_valid(i) { v.push((h64(sa.value(i).as_bytes()), (off + i as u64) as u32)); } }
        off += batch.num_rows() as u64;
    }
    Ok(v)
}

/// None on any read error, cancel, or >4G rows: a partial index would give wrong (missing) results.
pub(crate) fn build_exact(t: &Table, col: usize, job: &Job) -> Option<ExactIdx> {
    if t.total() > u32::MAX as u64 { return None; }
    let parts = (0..t.num_rgs()).into_par_iter().map(|g| {
        if job.cancelled() { return Err("cancelled".to_string()); }
        let r = exact_rg(t, g, col).map_err(|e| e.to_string());
        job.tick();
        r
    }).collect::<Result<Vec<_>, String>>().ok()?;
    job.set_phase("Merging exact index...");
    let mut all: Vec<(u64, u32)> = Vec::with_capacity(parts.iter().map(|p| p.len()).sum());
    for p in parts { all.extend(p); } // each part is freed as soon as it is moved, so memory stays at 16 bytes/row
    if job.cancelled() { return None; }
    job.set_phase("Sorting exact index (cannot be cancelled)...");
    all.par_sort_unstable();
    if job.cancelled() { return None; }
    Some(ExactIdx { pairs: all })
}

pub(crate) fn exact_path(t: &Table, col: usize) -> PathBuf { cache_path(t).with_extension(format!("c{col}.exi")) }

pub(crate) fn save_exact(t: &Table, col: usize, ix: &ExactIdx) {
    let path = exact_path(t, col);
    let Ok(f) = File::create(&path) else { return };
    let mut w = BufWriter::with_capacity(1 << 20, f);
    let ok = (|| -> std::io::Result<()> {
        w.write_all(b"EXI1")?;
        w.write_all(&(ix.pairs.len() as u64).to_le_bytes())?;
        for p in &ix.pairs { w.write_all(&p.0.to_le_bytes())?; }
        for p in &ix.pairs { w.write_all(&p.1.to_le_bytes())?; }
        w.flush()
    })();
    if ok.is_err() { drop(w); let _ = std::fs::remove_file(&path); } // never leave a partial index behind
}

pub(crate) fn load_exact(t: &Table, col: usize) -> Option<ExactIdx> {
    let path = exact_path(t, col);
    let len = std::fs::metadata(&path).ok()?.len();
    let mut f = BufReader::with_capacity(1 << 20, File::open(&path).ok()?);
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    if &head[..4] != b"EXI1" { return None; }
    let n = u64::from_le_bytes(head[4..12].try_into().ok()?) as usize;
    if len != 12 + n as u64 * 12 { return None; }
    let mut pairs = vec![(0u64, 0u32); n];
    let mut b8 = [0u8; 8];
    for p in pairs.iter_mut() { f.read_exact(&mut b8).ok()?; p.0 = u64::from_le_bytes(b8); }
    let mut b4 = [0u8; 4];
    for p in pairs.iter_mut() { f.read_exact(&mut b4).ok()?; p.1 = u32::from_le_bytes(b4); }
    Some(ExactIdx { pairs })
}
