//! Sample-data generator for screenshots and trials:
//!   cargo run --release --example sample -- sample.parquet 1000000
//! Writes 20 columns of invented people, places and orders. Every name, address, phone number and
//! amount is made up. Rows are in creation order, so sorting by `created` can stop early, and the
//! email column has a bloom filter, so exact lookups on it can skip row groups.
use arrow::{array::*, datatypes::*, record_batch::RecordBatch};
use parquet::{arrow::ArrowWriter, basic::Compression, file::properties::WriterProperties};
use std::{fs::File, sync::Arc};

const FIRST: &[&str] = &[
    "Amara", "Luca", "Mei", "Jonas", "Priya", "Tomas", "Sofia", "Kofi", "Elena", "Hiro",
    "Nadia", "Oscar", "Leila", "Mateo", "Ingrid", "Samir", "Clara", "Dmitri", "Aisha", "Pavel",
    "Rosa", "Kwame", "Freya", "Diego", "Yuki", "Anders", "Zara", "Marek", "Imani", "Felix",
    "Lucia", "Omar", "Greta", "Ravi", "Alma", "Tariq", "Hana", "Bruno", "Nia", "Stefan",
];
const LAST: &[&str] = &[
    "Okafor", "Ferrari", "Tanaka", "Weber", "Nair", "Berg", "Alvarez", "Mensah", "Petrova", "Sato",
    "Haddad", "Lindqvist", "Rahman", "Costa", "Larsen", "Khan", "Dubois", "Ivanov", "Bello", "Novak",
    "Silva", "Owusu", "Nilsen", "Herrera", "Kimura", "Holm", "Ahmed", "Kowalski", "Njoroge", "Bauer",
    "Moreno", "Farah", "Fischer", "Patel", "Soto", "Aziz", "Mori", "Rossi", "Adeyemi", "Vogel",
];
// city, country, currency, region, phone prefix
const PLACES: &[(&str, &str, &str, &str, &str)] = &[
    ("Lisbon", "Portugal", "EUR", "Europe", "+351"),
    ("Porto", "Portugal", "EUR", "Europe", "+351"),
    ("Berlin", "Germany", "EUR", "Europe", "+49"),
    ("Munich", "Germany", "EUR", "Europe", "+49"),
    ("Madrid", "Spain", "EUR", "Europe", "+34"),
    ("Paris", "France", "EUR", "Europe", "+33"),
    ("Rome", "Italy", "EUR", "Europe", "+39"),
    ("Amsterdam", "Netherlands", "EUR", "Europe", "+31"),
    ("Dublin", "Ireland", "EUR", "Europe", "+353"),
    ("London", "United Kingdom", "GBP", "Europe", "+44"),
    ("Nairobi", "Kenya", "KES", "Africa", "+254"),
    ("Lagos", "Nigeria", "NGN", "Africa", "+234"),
    ("Cape Town", "South Africa", "ZAR", "Africa", "+27"),
    ("Cairo", "Egypt", "EGP", "Africa", "+20"),
    ("Toronto", "Canada", "CAD", "North America", "+1"),
    ("Austin", "United States", "USD", "North America", "+1"),
    ("Denver", "United States", "USD", "North America", "+1"),
    ("Seattle", "United States", "USD", "North America", "+1"),
    ("Mexico City", "Mexico", "MXN", "North America", "+52"),
    ("Sao Paulo", "Brazil", "BRL", "South America", "+55"),
    ("Buenos Aires", "Argentina", "ARS", "South America", "+54"),
    ("Tokyo", "Japan", "JPY", "Asia", "+81"),
    ("Seoul", "South Korea", "KRW", "Asia", "+82"),
    ("Sydney", "Australia", "AUD", "Oceania", "+61"),
];
const DOMAINS: &[&str] = &["example.com", "example.org", "example.net"];
const STATUS: &[&str] = &["Active", "Active", "Active", "Active", "Active", "Active", "Paused", "Paused", "Pending", "Closed"];
const CHANNEL: &[&str] = &["Web", "Referral", "Partner", "Email", "Event"];
const PLAN: &[&str] = &["Solo monthly", "Team monthly", "Business annual", "Enterprise"];
const TAGS: &[&str] = &["new", "priority", "trial", "newsletter,priority", "beta", "renewal,priority", "newsletter", "beta,trial"];
const NOTES: &[&str] = &[
    "", "", "Asked about invoices", "Moved to the annual plan", "Requested a demo", "",
    "Contact by email only", "Referred by a partner", "", "Prefers a morning call",
];

const BASE: i64 = 1_640_995_200; // 2022-01-01 00:00:00 UTC, in seconds
const SPAN: i64 = 110_332_800; // 1277 days: the data runs to the end of June 2025

/// A deterministic pseudo-random number per row and column, so the same input gives the same file.
fn r(i: i64, salt: u64) -> u64 {
    let mut x = (i as u64).wrapping_mul(31).wrapping_add(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().unwrap_or("sample.parquet".into());
    let n: i64 = a.next().and_then(|x| x.parse().ok()).unwrap_or(1_000_000);
    let ts = |i: i64| -> i64 { (BASE + (i as i128 * SPAN as i128 / n as i128) as i64 + (r(i, 8) % 3600) as i64) * 1000 };

    let tm = DataType::Timestamp(TimeUnit::Millisecond, None);
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("first_name", DataType::Utf8, false),
        Field::new("last_name", DataType::Utf8, false),
        Field::new("email", DataType::Utf8, false),
        Field::new("phone", DataType::Utf8, false),
        Field::new("city", DataType::Utf8, false),
        Field::new("country", DataType::Utf8, false),
        Field::new("postal_code", DataType::Utf8, false),
        Field::new("amount", DataType::Float64, false),
        Field::new("currency", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("channel", DataType::Utf8, false),
        Field::new("plan", DataType::Utf8, false),
        Field::new("created", tm.clone(), false),
        Field::new("updated", tm, false),
        Field::new("score", DataType::Int32, false),
        Field::new("region", DataType::Utf8, false),
        Field::new("active", DataType::Boolean, false),
        Field::new("tags", DataType::Utf8, false),
        Field::new("notes", DataType::Utf8, false),
    ]));
    let props = WriterProperties::builder()
        .set_max_row_group_size(100_000)
        .set_compression(Compression::SNAPPY)
        .set_dictionary_enabled(true)
        .set_column_bloom_filter_enabled("email".into(), true)
        .build();
    let mut w = ArrowWriter::try_new(File::create(&path).unwrap(), schema.clone(), Some(props)).unwrap();

    let first = |i: i64| FIRST[(r(i, 1) % FIRST.len() as u64) as usize];
    let last = |i: i64| LAST[(r(i, 2) % LAST.len() as u64) as usize];
    let place = |i: i64| PLACES[(r(i, 3) % PLACES.len() as u64) as usize];

    let step = 100_000;
    let mut start = 0;
    while start < n {
        let end = (start + step).min(n);
        let rows = start..end;
        let cols: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from_iter_values(rows.clone())),
            Arc::new(StringArray::from_iter_values(rows.clone().map(first))),
            Arc::new(StringArray::from_iter_values(rows.clone().map(last))),
            Arc::new(StringArray::from_iter_values(rows.clone().map(|i| {
                format!("{}.{}{}@{}", first(i).to_lowercase(), last(i).to_lowercase(), r(i, 4) % 1000, DOMAINS[(r(i, 5) % DOMAINS.len() as u64) as usize])
            }))),
            Arc::new(StringArray::from_iter_values(rows.clone().map(|i| format!("{} 555 {:04}", place(i).4, r(i, 12) % 10_000)))),
            Arc::new(StringArray::from_iter_values(rows.clone().map(|i| place(i).0))),
            Arc::new(StringArray::from_iter_values(rows.clone().map(|i| place(i).1))),
            Arc::new(StringArray::from_iter_values(rows.clone().map(|i| format!("{:05}", r(i, 13) % 100_000)))),
            Arc::new(Float64Array::from_iter_values(rows.clone().map(|i| (r(i, 7) % 1_000_000) as f64 / 100.0))),
            Arc::new(StringArray::from_iter_values(rows.clone().map(|i| place(i).2))),
            Arc::new(StringArray::from_iter_values(rows.clone().map(|i| STATUS[(r(i, 6) % STATUS.len() as u64) as usize]))),
            Arc::new(StringArray::from_iter_values(rows.clone().map(|i| CHANNEL[(r(i, 14) % CHANNEL.len() as u64) as usize]))),
            Arc::new(StringArray::from_iter_values(rows.clone().map(|i| PLAN[(r(i, 15) % PLAN.len() as u64) as usize]))),
            Arc::new(TimestampMillisecondArray::from_iter_values(rows.clone().map(ts))),
            Arc::new(TimestampMillisecondArray::from_iter_values(rows.clone().map(|i| ts(i) + (r(i, 9) % (30 * 86_400)) as i64 * 1000))),
            Arc::new(Int32Array::from_iter_values(rows.clone().map(|i| (r(i, 10) % 101) as i32))),
            Arc::new(StringArray::from_iter_values(rows.clone().map(|i| place(i).3))),
            Arc::new(BooleanArray::from(rows.clone().map(|i| r(i, 11) % 10 != 0).collect::<Vec<bool>>())),
            Arc::new(StringArray::from_iter_values(rows.clone().map(|i| TAGS[(r(i, 16) % TAGS.len() as u64) as usize]))),
            Arc::new(StringArray::from_iter_values(rows.clone().map(|i| NOTES[(r(i, 17) % NOTES.len() as u64) as usize]))),
        ];
        w.write(&RecordBatch::try_new(schema.clone(), cols).unwrap()).unwrap();
        start = end;
    }
    w.close().unwrap();
    println!("wrote {n} rows to {path}");
}
