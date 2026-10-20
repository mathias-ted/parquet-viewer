//! Open a file, or a folder of Parquet files, as one table: footers, row-group offsets, block loading.
//! Row groups are numbered globally across files; `rg_file` maps a global row group to (file, local index).
use crate::prelude::*;

pub(crate) type Res<T> = Result<T, Box<dyn std::error::Error>>;

pub(crate) const BLOCK: u64 = 2048; // rows decoded per load; bigger = fewer loads while scrolling

pub(crate) struct FileInfo {
    pub(crate) path: PathBuf,
    pub(crate) arm: ArrowReaderMetadata,
}

pub(crate) struct Table {
    pub(crate) path: PathBuf,                  // the file or folder that was opened
    pub(crate) files: Vec<FileInfo>,
    pub(crate) rg_file: Vec<(usize, usize)>,   // global row group -> (file index, row group within file)
    pub(crate) names: Vec<String>,
    pub(crate) starts: Arc<Vec<u64>>,          // cumulative row offsets per global row group, len = row groups + 1
    pub(crate) flat: bool,                     // every file has a flat schema (enables pruning)
}

impl Table {
    pub(crate) fn total(&self) -> u64 { *self.starts.last().unwrap() }
    pub(crate) fn num_rgs(&self) -> usize { self.rg_file.len() }
    pub(crate) fn locate(&self, id: u64) -> (usize, u64) {
        let g = self.starts.partition_point(|&s| s <= id) - 1;
        (g, id - self.starts[g])
    }
    pub(crate) fn schema(&self) -> &arrow::datatypes::SchemaRef { self.files[0].arm.schema() }
    pub(crate) fn rg_meta(&self, g: usize) -> &parquet::file::metadata::RowGroupMetaData {
        let (f, l) = self.rg_file[g];
        self.files[f].arm.metadata().row_group(l)
    }
    /// Reader for one global row group. `dm` = per-file metadata with a modified schema (dictionary hint), if any.
    pub(crate) fn builder(&self, g: usize, dm: Option<&[ArrowReaderMetadata]>) -> Res<ParquetRecordBatchReaderBuilder<File>> {
        let (f, l) = self.rg_file[g];
        let m = dm.map(|d| &d[f]).unwrap_or(&self.files[f].arm).clone();
        Ok(ParquetRecordBatchReaderBuilder::new_with_metadata(File::open(&self.files[f].path)?, m).with_row_groups(vec![l]))
    }
}

fn collect_parquet(dir: &std::path::Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() { collect_parquet(&p, out)?; }
        else if matches!(p.extension().and_then(|x| x.to_str()), Some("parquet") | Some("pq")) { out.push(p); }
    }
    Ok(())
}

fn same_schema(a: &Schema, b: &Schema) -> bool {
    a.fields().len() == b.fields().len() && a.fields().iter().zip(b.fields()).all(|(x, y)| x.name() == y.name() && x.data_type() == y.data_type())
}

pub(crate) fn open(path: PathBuf) -> Res<Table> {
    let mut paths = vec![];
    if path.is_dir() { collect_parquet(&path, &mut paths)?; paths.sort(); } else { paths.push(path.clone()); }
    if paths.is_empty() { return Err("no .parquet or .pq files found in that folder".into()); }
    let files = paths.par_iter().map(|p| -> Result<FileInfo, String> { // footers only, read in parallel
        let f = File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let arm = ArrowReaderMetadata::load(&f, ArrowReaderOptions::new().with_page_index(true)).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(FileInfo { path: p.clone(), arm })
    }).collect::<Result<Vec<_>, String>>()?;
    for f in &files[1..] {
        if !same_schema(files[0].arm.schema(), f.arm.schema()) {
            return Err(format!("schema of {} differs from {}", f.path.display(), files[0].path.display()).into());
        }
    }
    let names = files[0].arm.schema().fields().iter().map(|f| f.name().clone()).collect::<Vec<_>>();
    let flat = files.iter().all(|f| f.arm.parquet_schema().num_columns() == names.len());
    let (mut rg_file, mut starts) = (vec![], vec![0u64]);
    for (fi, f) in files.iter().enumerate() {
        let md = f.arm.metadata();
        for l in 0..md.num_row_groups() {
            rg_file.push((fi, l));
            let last = *starts.last().unwrap();
            starts.push(last + md.row_group(l).num_rows() as u64);
        }
    }
    Ok(Table { path, files, rg_file, names, starts: Arc::new(starts), flat })
}

/// Late materialization: decode one 256-row block of one row group, only when it becomes visible.
pub(crate) fn load_block(t: &Table, g: usize, blk: u64) -> Res<Vec<Vec<String>>> {
    let n = t.starts[g + 1] - t.starts[g];
    let (s, len) = (blk * BLOCK, BLOCK.min(n - blk * BLOCK));
    let mut sel = vec![];
    if s > 0 { sel.push(RowSelector::skip(s as usize)); }
    sel.push(RowSelector::select(len as usize));
    let b = t.builder(g, None)?;
    let r = b.with_row_selection(RowSelection::from(sel)).with_batch_size(BLOCK as usize).build()?;
    let opts = FormatOptions::default().with_null("null");
    let mut rows = vec![];
    for batch in r {
        let batch = batch?;
        let f: Vec<_> = batch.columns().iter().map(|c| ArrayFormatter::try_new(c.as_ref(), &opts)).collect::<Result<_, _>>()?;
        for i in 0..batch.num_rows() { rows.push(f.iter().map(|x| x.value(i).to_string()).collect()); }
    }
    Ok(rows)
}
