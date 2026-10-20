//! Unit tests (run with `cargo test`).
use crate::prelude::*;

#[test]
fn trigram_no_false_negatives() {
    let mut bits = vec![0u8; BYTES];
    let vals = ["Hello World", "user123@Example.com", "caf\u{e9}-ascii-part"];
    for v in vals { add_tris(&mut bits, v.as_bytes()); }
    for v in vals {
        let l = v.to_ascii_lowercase();
        let b = l.as_bytes();
        for i in 0..b.len() {
            for j in i + 3..=b.len() {
                if let Ok(sub) = std::str::from_utf8(&b[i..j]) {
                    if sub.is_ascii() {
                        assert!(may_contain(&bits, sub.as_bytes()), "missing {sub}");
                        assert!(may_contain(&bits, sub.to_ascii_uppercase().as_bytes()), "case {sub}");
                    }
                }
            }
        }
    }
}

#[test]
fn trigram_rejects_absent() {
    let mut bits = vec![0u8; BYTES];
    add_tris(&mut bits, b"hello world");
    assert!(!may_contain(&bits, b"zzzqqq"));
}

#[test]
fn fast_contains_matches_rows_correctly() {
    let sa = StringArray::from(vec!["Needle.com", "other", "xnee", "dle", "NEEDLE", "a-needle-b", ""]);
    let ac = AhoCorasick::builder().ascii_case_insensitive(true).build(["needle"]).unwrap();
    let m = fast_contains(&sa, &ac);
    let got: Vec<bool> = (0..m.len()).map(|i| m.value(i)).collect();
    assert_eq!(got, vec![true, false, false, false, true, true, false]); // "xnee" followed by "dle" spans two rows and must not match
}

#[test]
fn range_parsing_and_matching() {
    let r = parse_range(">=5").unwrap();
    assert!(r.contains(5.0) && r.contains(9.0) && !r.contains(4.9));
    let r = parse_range(">5").unwrap();
    assert!(!r.contains(5.0) && r.contains(5.1));
    let r = parse_range("<2.5").unwrap();
    assert!(r.contains(2.4) && !r.contains(2.5));
    let r = parse_range("10..20").unwrap();
    assert!(r.contains(10.0) && r.contains(20.0) && !r.contains(20.1));
    assert!(parse_range("hello").is_none() && parse_range(">x").is_none() && parse_range("123").is_none());
}

#[test]
fn local_top_orders_and_limits() {
    use arrow::array::Int32Array;
    let v: ArrayRef = Arc::new(Int32Array::from(vec![5, 1, 4, 2, 3]));
    let ids = vec![10, 11, 12, 13, 14];
    assert_eq!(local_top(v.clone(), ids.clone(), 3, false).unwrap().1, vec![11, 13, 14]);
    assert_eq!(local_top(v, ids, 3, true).unwrap().1, vec![10, 12, 14]);
}

#[test]
fn exact_lookup_finds_all_rows() {
    let vals = ["a.com", "b.com", "a.com", "c.com", "a.com"];
    let mut all: Vec<(u64, u32)> = vals.iter().enumerate().map(|(i, v)| (h64(v.as_bytes()), i as u32)).collect();
    all.sort_unstable();
    let ix = ExactIdx { pairs: all };
    assert_eq!(exact_lookup(&ix, "a.com"), vec![0, 2, 4]);
    assert_eq!(exact_lookup(&ix, "c.com"), vec![3]);
    assert!(exact_lookup(&ix, "zzz").is_empty());
}

#[test]
fn escape_like_chars() {
    assert_eq!(esc("a%b_c\\"), "a\\%b\\_c\\\\");
}

#[test]
fn txt_quotes_values_with_colons_only_when_several_columns() {
    let names = vec!["name".to_string(), "link".to_string()];
    let row = vec!["a.com".to_string(), "https://x.com/p".to_string()];
    assert_eq!(format_row(ExportFmt::Txt, &names, &row), "a.com:\"https://x.com/p\"");
    let plain = vec!["a.com".to_string(), "b".to_string()];
    assert_eq!(format_row(ExportFmt::Txt, &names, &plain), "a.com:b");
    let one = vec!["https://x.com".to_string()];
    assert_eq!(format_row(ExportFmt::Txt, &names[..1], &one), "https://x.com"); // single column: no separator, no quotes
    let q = vec!["say \"hi\": ok".to_string(), "z".to_string()];
    assert_eq!(format_row(ExportFmt::Txt, &names, &q), "\"say \"\"hi\"\": ok\":z");
}

#[test]
fn kth_key_reads_the_boundary_value() {
    use arrow::array::{Float64Array, Int32Array, StringArray};
    let ints: ArrayRef = Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5]));
    assert_eq!(kth_key(&ints, 3), Some(Key::I(3)));
    assert_eq!(kth_key(&ints, 6), None); // fewer than k values: no threshold yet
    let floats: ArrayRef = Arc::new(Float64Array::from(vec![0.5, 1.5, 2.5]));
    assert_eq!(kth_key(&floats, 2), Some(Key::F(1.5)));
    let strs: ArrayRef = Arc::new(StringArray::from(vec!["a", "b", "c"]));
    assert_eq!(kth_key(&strs, 1), Some(Key::S(b"a".to_vec())));
    let with_null: ArrayRef = Arc::new(Int32Array::from(vec![Some(1), None]));
    assert_eq!(kth_key(&with_null, 2), None); // the k-th entry is a null: not a usable threshold
}

#[test]
fn key_ordering_matches_value_ordering() {
    assert!(Key::I(2) < Key::I(10));
    assert!(Key::F(-1.0) < Key::F(0.5));
    assert!(Key::S(b"abc".to_vec()) < Key::S(b"abd".to_vec()));
}

#[test]
fn compression_names_are_shortened() {
    assert_eq!(short("ZSTD(ZstdLevel(1))".to_string()), "ZSTD");
    assert_eq!(short("SNAPPY".to_string()), "SNAPPY");
}
