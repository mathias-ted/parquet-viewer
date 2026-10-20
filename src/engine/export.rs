//! Export results as CSV, colon-separated TXT or JSON, for chosen columns.
//! Search results of any size are streamed row group by row group (bounded memory).
use crate::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ExportFmt { #[default] Csv, Txt, Json }
impl ExportFmt {
    pub(crate) fn ext(self) -> &'static str { match self { ExportFmt::Csv => "csv", ExportFmt::Txt => "txt", ExportFmt::Json => "json" } }
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}
/// TXT cell: a value containing ':' is quoted ("" for inner quotes) when columns are colon-separated.
fn txt_cell(s: &str, quote_colons: bool) -> String {
    let s = s.replace(['\n', '\r'], " ");
    if quote_colons && s.contains(':') { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}
fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// One serialized row (no trailing newline / comma).
pub(crate) fn format_row(fmt: ExportFmt, names: &[String], cells: &[String]) -> String {
    match fmt {
        ExportFmt::Csv => cells.iter().map(|s| csv_field(s)).collect::<Vec<_>>().join(","),
        ExportFmt::Txt => { let multi = cells.len() > 1; cells.iter().map(|s| txt_cell(s, multi)).collect::<Vec<_>>().join(":") }
        ExportFmt::Json => {
            let mut o = String::from("{");
            for (i, (n, c)) in names.iter().zip(cells).enumerate() {
                if i > 0 { o.push(','); }
                o.push_str(&json_str(n));
                o.push(':');
                o.push_str(&json_str(c));
            }
            o.push('}');
            o
        }
    }
}

pub(crate) struct Sink<W: Write> { w: W, fmt: ExportFmt, first: bool }
impl<W: Write> Sink<W> {
    pub(crate) fn new(mut w: W, fmt: ExportFmt, names: &[String]) -> std::io::Result<Self> {
        match fmt {
            ExportFmt::Json => write!(w, "[")?,
            _ => writeln!(w, "{}", format_row(fmt, names, names))?, // header line
        }
        Ok(Self { w, fmt, first: true })
    }
    pub(crate) fn line(&mut self, l: &str) -> std::io::Result<()> {
        if self.fmt == ExportFmt::Json {
            if !self.first { write!(self.w, ",")?; }
            self.first = false;
            write!(self.w, "\n{l}")
        } else {
            writeln!(self.w, "{l}")
        }
    }
    pub(crate) fn finish(mut self) -> std::io::Result<()> {
        if self.fmt == ExportFmt::Json { writeln!(self.w, "\n]")?; }
        self.w.flush()
    }
}

fn reader_for(t: &Table, g: usize, cols: &[usize]) -> Res<parquet::arrow::arrow_reader::ParquetRecordBatchReader> {
    let b = t.builder(g, None)?;
    let mask = ProjectionMask::roots(b.parquet_schema(), cols.iter().copied()); // read only the chosen columns
    Ok(b.with_projection(mask).with_batch_size(8192).build()?)
}

/// Ascending `ids` within one row group -> serialized lines.
fn rg_lines(t: &Table, g: usize, ids: &[u64], fmt: ExportFmt, names: &[String], cols: &[usize]) -> Res<Vec<String>> {
    let opts = FormatOptions::default().with_null("");
    let (mut off, mut p, mut out) = (t.starts[g], 0usize, Vec::with_capacity(ids.len()));
    for batch in reader_for(t, g, cols)? {
        let batch = batch?;
        let end = off + batch.num_rows() as u64;
        if p < ids.len() && ids[p] < end {
            let f: Vec<_> = batch.columns().iter().map(|c| ArrayFormatter::try_new(c.as_ref(), &opts)).collect::<Result<_, _>>()?;
            while p < ids.len() && ids[p] < end {
                let i = (ids[p] - off) as usize;
                let cells: Vec<String> = f.iter().map(|x| x.value(i).to_string()).collect();
                out.push(format_row(fmt, names, &cells));
                p += 1;
            }
        }
        off = end;
        if p >= ids.len() { break; }
    }
    Ok(out)
}

/// Any `ids` of one row group -> (id, selected cells), for views that are not in file order (sorted results).
fn fetch_rows(t: &Table, g: usize, want: &HashSet<u64>, cols: &[usize]) -> Res<Vec<(u64, Vec<String>)>> {
    let opts = FormatOptions::default().with_null("");
    let (mut off, mut out) = (t.starts[g], vec![]);
    for batch in reader_for(t, g, cols)? {
        let batch = batch?;
        let n = batch.num_rows();
        let hit: Vec<usize> = (0..n).filter(|&i| want.contains(&(off + i as u64))).collect();
        if !hit.is_empty() {
            let f: Vec<_> = batch.columns().iter().map(|c| ArrayFormatter::try_new(c.as_ref(), &opts)).collect::<Result<_, _>>()?;
            for i in hit { out.push((off + i as u64, f.iter().map(|x| x.value(i).to_string()).collect())); }
        }
        off += n as u64;
    }
    Ok(out)
}

/// Exports every row id in `ids` (no row limit). `cols` must be ascending column indices.
/// Progress counts row groups as each one finishes; Cancel is honoured inside every phase.
/// On any failure or cancel, the partly written file is deleted.
pub(crate) fn export_ids(t: &Table, ids: &[u64], path: &std::path::Path, fmt: ExportFmt, cols: &[usize], job: &Job) -> Res<usize> {
    let r = export_ids_inner(t, ids, path, fmt, cols, job);
    if r.is_err() { let _ = std::fs::remove_file(path); } // the writer is dropped by now, so the file can be removed
    r
}

fn export_ids_inner(t: &Table, ids: &[u64], path: &std::path::Path, fmt: ExportFmt, cols: &[usize], job: &Job) -> Res<usize> {
    let names: Vec<String> = cols.iter().map(|&i| t.names[i].clone()).collect();
    let mut s = Sink::new(BufWriter::with_capacity(1 << 20, File::create(path)?), fmt, &names)?;
    let mut n = 0usize;
    if ids.windows(2).all(|w| w[0] <= w[1]) {
        // file order (search results): stream a few row groups at a time, memory stays bounded
        let mut groups: Vec<(usize, usize, usize)> = vec![];
        let mut i = 0;
        while i < ids.len() {
            let g = t.locate(ids[i]).0;
            let end = t.starts[g + 1];
            let j = i + ids[i..].partition_point(|&x| x < end);
            groups.push((g, i, j));
            i = j;
        }
        job.total.store(groups.len().max(1), Relaxed);
        for chunk in groups.chunks(rayon::current_num_threads().clamp(1, 6)) {
            let parts = chunk.par_iter().map(|&(g, a, b)| {
                if job.cancelled() { return Err("cancelled".to_string()); }
                let r = rg_lines(t, g, &ids[a..b], fmt, &names, cols).map_err(|e| e.to_string());
                job.tick(); // one row group finished
                r
            }).collect::<Result<Vec<_>, String>>()?;
            for lines in parts { for l in lines { s.line(&l)?; n += 1; } }
        }
    } else {
        // sorted view: keep the view's order (these views are small, top-k)
        let mut groups: HashMap<usize, HashSet<u64>> = HashMap::new();
        for &id in ids { groups.entry(t.locate(id).0).or_default().insert(id); }
        job.total.store(groups.len().max(1), Relaxed);
        let parts = groups.into_par_iter().map(|(g, want)| {
            if job.cancelled() { return Err("cancelled".to_string()); }
            let r = fetch_rows(t, g, &want, cols).map_err(|e| e.to_string());
            job.tick();
            r
        }).collect::<Result<Vec<_>, String>>()?;
        let rows: HashMap<u64, Vec<String>> = parts.into_iter().flatten().collect();
        for id in ids { if let Some(r) = rows.get(id) { s.line(&format_row(fmt, &names, r))?; n += 1; } }
    }
    if job.cancelled() { return Err("cancelled".into()); }
    s.finish()?;
    Ok(n)
}

/// SQL results: the rows shown (at most SQL_MAX_ROWS are kept in memory).
pub(crate) fn export_sql(r: &SqlResult, path: &std::path::Path, fmt: ExportFmt, cols: &[usize]) -> Res<usize> {
    let names: Vec<String> = cols.iter().map(|&i| r.cols[i].clone()).collect();
    let mut s = Sink::new(BufWriter::with_capacity(1 << 20, File::create(path)?), fmt, &names)?;
    for row in &r.rows {
        let cells: Vec<String> = cols.iter().map(|&i| row.get(i).cloned().unwrap_or_default()).collect();
        s.line(&format_row(fmt, &names, &cells))?;
    }
    s.finish()?;
    Ok(r.rows.len())
}
