<p align="center"><img src="assets/icon.png" width="96" alt="Parquet Viewer icon"></p>

# Parquet Viewer

A fast desktop viewer and search tool for very large Parquet files, or whole folders of them.
One native program that runs locally and needs no installer and no runtime.

## Features

- **Instant open.** Only the file footers are read, so even a folder with hundreds of millions of rows opens in well under a second.
- **Folders as one table.** Browse, search, sort, export and query many files together. Subfolders are included.
- **Fast search.** Contains, equals and numeric ranges. Results stream in as they are found, and any search can be cancelled. The search on screen appears as a chip that clears it with one click.
- **Reads only what it needs.** Column projection, row-group min/max statistics, bloom filters and a dictionary fast path skip as much data as the file allows.
- **Optional indexes.** A trigram text index and an exact-value index, built once and reused. Exact lookups on an indexed column take milliseconds.
- **Sorting.** Sort any column by its heading. After a search it sorts the search results, and it stops early using the file's own statistics when it can.
- **Table view.** Each column heading shows the column type. **Columns** hides columns from the table; hidden columns are still searched and exported.
- **Schema window.** Columns with their types, compression, encodings and value ranges, plus file and row-group sizes.
- **SQL.** Query the open file or folder as a table named `data` (DataFusion). Memory use is capped, so a heavy query fails with a message instead of crashing the program.
- **Double-click to open (Windows).** The **Default app...** button adds the program to the Open with list for `.parquet` and `.pq` files and opens the Windows settings where it can be chosen as the default.
- **Export and convert.** CSV, TXT or JSON, with the selected columns. With no search active the whole file is written, so it also converts Parquet to other formats. Search, sort and SQL results export in full, with no row limit.
- **Safe on big data.** Long tasks show progress and can be cancelled. Index builds check free memory first.

## Install

Download `parquet-viewer.exe` from the [Releases](../../releases) page and run it. Windows 10 or 11, 64-bit.

Other systems: build from source (see below). Only Windows has been tested.

## Usage

```
parquet-viewer [file-or-folder]
```

A file or folder can also be opened with the **Open...** and **Open folder...** buttons, or dropped onto the window.

### Opening a folder

All `.parquet` and `.pq` files in the folder and its subfolders open as one table.

**Every file must have the same columns, with the same names and types.**
If one differs, the folder is not opened and the message names the file. Values encoded in folder names (such as `year=2024/`) are not added as columns.

### Searching

Pick a column, or leave **All columns**, choose **Contains** or **Equals**, then type and press Enter or **Search**. The search stays on screen as a chip; the **x** on the chip clears it.

| Input | Matches |
|---|---|
| text | rows containing the text, ignoring case |
| text with **Equals** selected | values equal to the text, case-sensitive |
| `>5`, `>=5`, `<10`, `<=10`, `5..10` | numeric ranges, on number columns |

### Sorting

Click a column heading, and again to reverse. After a search, the search results are sorted.
With no search, the table shows the first 1,000 rows in sorted order, not the whole file.

### SQL

The table is called `data` and refers to the open file or folder.

```sql
SELECT COUNT(*) FROM data
SELECT * FROM data WHERE some_column = 'x' LIMIT 100
SELECT some_column, COUNT(*) AS n FROM data GROUP BY some_column ORDER BY n DESC LIMIT 20
```

Names with capitals or spaces need double quotes. The table shows up to 100,000 rows of a result. **Export** writes all of them by running the query again.
Grouping or sorting an entire huge table can still be slow and use a lot of memory, so add `WHERE` or `LIMIT` where possible.

### Export

Choose a format and the columns to include. With a search, sort or SQL result on screen, those rows are written. With nothing active, **Export** writes the whole file, which converts it to the chosen format. The file is written in batches, so memory use stays small however large the data is.

| Format | Notes |
|---|---|
| CSV | comma-separated, quoted where needed |
| TXT | colon-separated. With more than one column, a value that contains a colon is quoted |
| JSON | a list of records, all values as text |

### Indexes

Searching works without indexes. They make repeated searches faster.

| Index | Speeds up | Memory |
|---|---|---|
| Text index | contains-search on text, when the word is uncommon | 32 KB per row group per text column |
| Exact index (selected column) | **Equals** lookups, nearly instant | about 16 bytes per row, held in memory while in use |

Indexes are saved in the system temporary folder and reused. If free memory is too low, the program says so instead of building them.

## Limitations

- Read-only: no editing or writing Parquet.
- Parquet only. Local files and folders only (no S3, HDFS or other remote storage).
- Nested columns (struct, list, map) are shown as text and are not used for skipping.
- Only the first 1,000 rows of a whole-file sort are shown.

## Troubleshooting

| Problem | What to do |
|---|---|
| Folder will not open: schema differs | Move the named file to another folder |
| "needs ... GB of free memory" | Close other programs or skip that index. Search still works |
| SQL error "Resources exhausted" | The query needs more memory than is free. Add `WHERE` or `LIMIT` |
| Program closes unexpectedly | Send `parquet-viewer-crash.log` from the temporary folder (Windows: `%TEMP%`) |

## Building from source

Requires a recent stable [Rust](https://rustup.rs). The first build takes several minutes.

```
cargo test
cargo run --release -- path/to/file_or_folder
cargo build --release --features hide-console
```

Use `--release`: a debug build is 10 to 50 times slower.
The last command produces the program to share: `target/release/parquet-viewer.exe` (Windows) or `target/release/parquet-viewer`.
Without `--features hide-console`, the program keeps a console window so crash messages are visible.

Debug switches (environment variables): `PQV_NODICT=1` turns off the dictionary read hint, and `PQV_NOEXACT=1` ignores the exact index.

### Project structure

```
src/
  main.rs         entry point, crash log
  prelude.rs      shared imports
  tests.rs        unit tests
  data/           reading Parquet
    table.rs        open a file or folder, global row-group numbering, block loading
    prune.rs        row-group skipping: statistics, bloom filters, number ranges
    schema.rs       file and column information for the Schema window
  engine/         work done on the data
    search.rs       parallel scans, dictionary fast path, fast text matching
    index.rs        text index and exact-value index
    sort.rs         top-k sorting, statistics-driven early stop
    sql.rs          SQL box (DataFusion), memory cap, streaming export
    export.rs       CSV, TXT and JSON export
    job.rs          progress and cancel for long tasks
  ui/
    app.rs          the window (egui)
    theme.rs        colours, spacing and the bundled font
    widgets.rs      filter chip, message box, progress card, scrollbar
    assoc.rs        Open with / default app registration (Windows)
assets/           icon, fonts (DM Sans)
```

## Contributing

Issues and pull requests are welcome. `cargo test` must pass; CI runs it on every push and builds the Windows program.
Commit `Cargo.lock`, since this is a program and not a library.

## License

MIT. See [LICENSE](LICENSE).

The bundled DM Sans font is licensed under the SIL Open Font License; see `assets/fonts/OFL.txt`.
