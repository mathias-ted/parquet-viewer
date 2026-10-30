# Parquet Viewer 0.1.1

A fast desktop viewer and search tool for very large Parquet files, or whole folders of them.

## What's new since 0.1.0

- **New interface.** A dark theme, the active search shown as a chip, column types in the table headings, a **Columns** window to hide columns from the table, a progress card with Cancel for long tasks, and clear messages for errors and limits.
- **Export a whole file.** With no search active, **Export** writes the entire file as CSV, TXT or JSON, so it also converts Parquet to other formats.

## Download

`parquet-viewer.exe` (Windows 10 or 11, 64-bit). A single file: no installer and no runtime needed.

## What's in this release

- **Open files and folders.** Only the file footers are read, so opening is quick even for hundreds of millions of rows. A folder (with its subfolders) opens as one table.
- **Search.** Contains, exact match and numeric ranges (`>5`, `5..10`). Results appear as they are found, and any search can be cancelled.
- **Less reading.** Column projection, row-group min/max statistics, bloom filters and a dictionary fast path skip data the search does not need.
- **Indexes (optional).** A text index and an exact-value index, built once and reused. Exact lookups on an indexed column take milliseconds.
- **Sorting.** Click a column heading. After a search, the search results are sorted. Whole-file sorts stop early using file statistics when the data allows it.
- **Schema window.** Columns with their types, compression, encodings and value ranges, plus file and row-group sizes.
- **SQL.** Query the open file or folder as a table named `data`. Memory use is capped, so a heavy query ends with a message instead of crashing the program.
- **Open with / default app (Windows).** The **Default app...** button adds the program to the Open with list for `.parquet` and `.pq` files and opens the Windows settings, where it can be chosen as the default.
- **Export and convert.** CSV, TXT (colon-separated) or JSON, with the selected columns. With no search active the whole file is written, so it also converts Parquet to other formats. Search, sort and SQL results export in full, with no row limit.
- **Long tasks.** Progress bar and Cancel for searches, index builds, sorting and exports. Index builds check free memory first.
- **Crash log.** If the program stops unexpectedly it writes `parquet-viewer-crash.log` to the temporary folder.

## Good to know

- Speed depends on the disk and on how the file was written. Skipping data (statistics, bloom filters, indexes) only helps when the file allows it. A search that has to read everything is limited by how fast the disk can deliver it.
- In a folder, every file must have the same column names and types. Otherwise the folder is not opened and the message names the file.
- The exact index needs about 16 bytes of free memory per row while it is in use.

## Limitations

- Read-only. Parquet files only. Local files and folders only.
- Nested columns (struct, list, map) are shown as text and are not used for skipping.
- A whole-file sort shows the first 1,000 rows in order, not the entire file.
- A SQL result is shown up to 100,000 rows. Export writes all of them.
- Exporting a whole file reads it in order on one thread, so a very large file takes a while. Memory use stays small.
- Only tested on Windows.

## Reporting a problem

Open an issue describing the steps that led to it, and attach `parquet-viewer-crash.log` if one was written (Windows: `%TEMP%`).
