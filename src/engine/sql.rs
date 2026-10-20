//! SQL box backed by DataFusion: `data` = the open file, or a folder of parquet files.
use crate::prelude::*;

pub(crate) const SQL_MAX_ROWS: usize = 100_000;

pub(crate) struct SqlResult { pub(crate) cols: Vec<String>, pub(crate) rows: Vec<Vec<String>>, pub(crate) ms: f64, pub(crate) truncated: bool }

/// A query session with `data` registered. Memory is capped (half of what is free now): a query that needs
/// more spills to disk or stops with an error, instead of making the whole program run out of memory.
async fn session(path: &std::path::Path) -> Result<datafusion::prelude::SessionContext, String> {
    use datafusion::execution::runtime_env::RuntimeEnvBuilder;
    use datafusion::prelude::{ParquetReadOptions, SessionConfig, SessionContext};
    let limit = (free_memory() / 2).max(256 * 1024 * 1024) as usize;
    let runtime = RuntimeEnvBuilder::new().with_memory_limit(limit, 1.0).build().map_err(|e| e.to_string())?;
    let ctx = SessionContext::new_with_config_rt(SessionConfig::new(), Arc::new(runtime));
    // Read text as plain strings: the default "view" strings can keep whole file buffers alive inside a sort,
    // which uses a huge amount of memory. If this version does not know the setting, the error is ignored.
    let _ = ctx.sql("SET datafusion.execution.parquet.schema_force_view_types = false").await;
    let p = path.to_string_lossy().to_string();
    let ext = if path.is_file() { path.extension().map(|e| format!(".{}", e.to_string_lossy())) } else { None };
    let mut opts = ParquetReadOptions::default();
    if let Some(e) = &ext { opts.file_extension = e.as_str(); } // `file_extension` is a public field
    ctx.register_parquet("data", &p, opts).await.map_err(|e| e.to_string())?;
    Ok(ctx)
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    // big thread stacks: the SQL engine plans queries with deep recursion
    tokio::runtime::Builder::new_multi_thread().enable_all().thread_stack_size(32 * 1024 * 1024).build().map_err(|e| e.to_string())
}

/// Runs the query and keeps up to SQL_MAX_ROWS rows for display (`truncated` = there were more).
pub(crate) fn run_sql(path: PathBuf, sql: String) -> Result<SqlResult, String> {
    use datafusion::arrow::util::display::{ArrayFormatter as Fmt, FormatOptions as FOpts};
    let t0 = Instant::now();
    runtime()?.block_on(async {
        let ctx = session(&path).await?;
        let df = ctx.sql(&sql).await.map_err(|e| e.to_string())?;
        let cols: Vec<String> = df.schema().fields().iter().map(|f| f.name().clone()).collect();
        let df = df.limit(0, Some(SQL_MAX_ROWS + 1)).map_err(|e| e.to_string())?;
        let batches = df.collect().await.map_err(|e| e.to_string())?;
        let fo = FOpts::default().with_null("null");
        let mut rows: Vec<Vec<String>> = vec![];
        for b in &batches {
            let f: Vec<_> = b.columns().iter().map(|c| Fmt::try_new(c.as_ref(), &fo)).collect::<Result<_, _>>().map_err(|e| e.to_string())?;
            for i in 0..b.num_rows() { rows.push(f.iter().map(|x| x.value(i).to_string()).collect()); }
        }
        let truncated = rows.len() > SQL_MAX_ROWS;
        rows.truncate(SQL_MAX_ROWS);
        Ok(SqlResult { cols, rows, ms: t0.elapsed().as_secs_f64() * 1e3, truncated })
    })
}

/// Runs the query again and streams EVERY result row to a file: no row limit, memory stays bounded.
/// `cols` = indices of the result columns to write. On error or cancel the partial file is deleted.
pub(crate) fn export_sql_stream(path: PathBuf, sql: String, out: &std::path::Path, fmt: ExportFmt, cols: &[usize], job: &Job) -> Result<usize, String> {
    use datafusion::arrow::util::display::{ArrayFormatter as Fmt, FormatOptions as FOpts};
    use futures::StreamExt;
    let r: Result<usize, String> = runtime()?.block_on(async {
        let ctx = session(&path).await?;
        let df = ctx.sql(&sql).await.map_err(|e| e.to_string())?;
        let all: Vec<String> = df.schema().fields().iter().map(|f| f.name().clone()).collect();
        let names: Vec<String> = cols.iter().filter_map(|&i| all.get(i).cloned()).collect();
        let file = File::create(out).map_err(|e| e.to_string())?;
        let mut sink = Sink::new(BufWriter::with_capacity(1 << 20, file), fmt, &names).map_err(|e| e.to_string())?;
        let mut stream = df.execute_stream().await.map_err(|e| e.to_string())?;
        let fo = FOpts::default().with_null("");
        let mut n = 0usize;
        while let Some(batch) = stream.next().await {
            if job.cancelled() { return Err("cancelled".to_string()); }
            let b = batch.map_err(|e| e.to_string())?;
            let f: Vec<_> = b.columns().iter().map(|c| Fmt::try_new(c.as_ref(), &fo)).collect::<Result<_, _>>().map_err(|e| e.to_string())?;
            for i in 0..b.num_rows() {
                let cells: Vec<String> = cols.iter().filter_map(|&c| f.get(c).map(|x| x.value(i).to_string())).collect();
                sink.line(&format_row(fmt, &names, &cells)).map_err(|e| e.to_string())?;
                n += 1;
            }
            job.set_phase(&format!("Exporting SQL results: {n} rows written"));
        }
        sink.finish().map_err(|e| e.to_string())?;
        Ok(n)
    });
    if r.is_err() { let _ = std::fs::remove_file(out); }
    r
}
