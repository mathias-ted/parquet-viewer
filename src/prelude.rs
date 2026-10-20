//! Shared imports for every module (one place to manage dependencies).
pub use aho_corasick::AhoCorasick;
pub use arrow::{
    array::{Array, ArrayRef, AsArray, BooleanArray, Scalar, StringArray},
    compute::{
        cast, concat, sort_to_indices, take, SortOptions,
        kernels::{boolean::or_kleene, cmp::eq, comparison::ilike},
    },
    datatypes::{DataType, Field, Float64Type, Int32Type, Schema},
    util::display::{ArrayFormatter, FormatOptions},
};
pub use eframe::egui;
pub use egui_extras::{Column, TableBuilder};
pub use parquet::{
    arrow::{
        arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder, RowSelection, RowSelector},
        ProjectionMask,
    },
    basic::{Encoding, PageType, Type as PType},
    data_type::ByteArray,
    file::{
        metadata::ColumnChunkMetaData,
        reader::FileReader,
        serialized_reader::SerializedFileReader,
        statistics::Statistics,
    },
};
pub use rayon::prelude::*;
pub use std::io::{BufReader, BufWriter, Read, Write};
pub use std::{
    collections::{HashMap, HashSet},
    fs::File,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    time::Instant,
};

pub(crate) use crate::{
    data::{prune::*, schema::*, table::*},
    engine::{export::*, index::*, job::*, search::*, sort::*, sql::*},
};
