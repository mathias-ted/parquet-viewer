//! Test-data generator for trying the viewer on large files: cargo run --release --example gen -- out.parquet 50000000
//! Columns: id (sorted i64), country (low-cardinality, dictionary), email (high-cardinality text),
//! amount (f64). Bloom filters on, 1M-row row groups, one planted needle row in the middle.
use arrow::{array::*, datatypes::*, record_batch::RecordBatch};
use parquet::{arrow::ArrowWriter, basic::Compression, file::properties::WriterProperties};
use std::{fs::File, sync::Arc};

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().unwrap_or("bench.parquet".into());
    let n: i64 = a.next().and_then(|x| x.parse().ok()).unwrap_or(50_000_000);
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("country", DataType::Utf8, false),
        Field::new("email", DataType::Utf8, false),
        Field::new("amount", DataType::Float64, false),
    ]));
    let props = WriterProperties::builder()
        .set_max_row_group_size(1_000_000)
        .set_compression(Compression::SNAPPY)
        .set_dictionary_enabled(true)
        .set_bloom_filter_enabled(true)
        .build();
    let mut w = ArrowWriter::try_new(File::create(&path).unwrap(), schema.clone(), Some(props)).unwrap();
    let step = 1_000_000;
    let mut start = 0;
    while start < n {
        let end = (start + step).min(n);
        let id = Int64Array::from_iter_values(start..end);
        let country = StringArray::from_iter_values((start..end).map(|i| format!("country-{}", i % 200)));
        let email = StringArray::from_iter_values((start..end).map(|i| {
            if i == n / 2 { "needle-xyz@example.com".to_string() }
            else { format!("user{:x}@example.com", (i as u64).wrapping_mul(0x9E3779B97F4A7C15) >> 20) }
        }));
        let amount = Float64Array::from_iter_values((start..end).map(|i| (i as f64 * 0.37) % 1000.0));
        let b = RecordBatch::try_new(schema.clone(), vec![Arc::new(id), Arc::new(country), Arc::new(email), Arc::new(amount)]).unwrap();
        w.write(&b).unwrap();
        start = end;
    }
    w.close().unwrap();
    println!("wrote {n} rows to {path}");
}
