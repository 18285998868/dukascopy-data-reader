//! Load Dukascopy public archive candles into column vectors.
//!
//! The reader consumes the directory layout produced by
//! `dukascopy-data-downloader`:
//!
//! ```text
//! {root}/dukascopy/{symbol}/{YYYY}/[{MM}/[{DD}/]]{BID|ASK}_candles_{min_1|hour_1|day_1}.bi5
//! ```
//!
//! `D1` is stored one file per year, `H1` one per month and `M1` one per day.
//! The path month is **zero-based** (`00` = January, `11` = December), matching
//! Dukascopy's datafeed; years and days stay one-based. A request expands its
//! date range into exactly those buckets; it never scans the symbol's full
//! history.
//!
//! # Example
//!
//! ```no_run
//! use chrono::NaiveDate;
//! use dukascopy_data_reader::{DataConfig, PriceType, Timeframe, load};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let config = DataConfig::new("/data/data", "EURUSD", Timeframe::H1)?
//!     .with_price_type(PriceType::Bid)
//!     .with_date_range(
//!         Some(NaiveDate::from_ymd_opt(2023, 6, 1).unwrap()),
//!         Some(NaiveDate::from_ymd_opt(2023, 6, 30).unwrap()),
//!     )?;
//! let loaded = load(&config)?;
//! println!("{} bars", loaded.columns().len());
//! println!("{} files missing", loaded.report().missing_files().len());
//! # Ok(())
//! # }
//! ```
//!
//! # Semantics
//!
//! - Time is Unix milliseconds at bar open, UTC.
//! - Prices are `f64`, decoded exactly from Dukascopy's five-decimal fixed
//!   point.
//! - Volume is the raw `u32` stored in the archive; it is **not** normalised.
//! - Missing archive files are reported, not fatal. A corrupt `.bi5` is a hard
//!   error carrying its path.
//! - Invalid OHLC rows are skipped and counted; duplicates are removed keeping
//!   the first bar.
//!
//! # Features
//!
//! - `python`: builds the PyO3 module and returns NumPy arrays. Disabled by
//!   default.

mod columns;
mod config;
mod decode;
mod error;
mod gaps;
mod layout;
mod load;
mod report;
mod types;

#[cfg(feature = "python")]
mod python;

pub use columns::{ColumnParts, KlineColumns};
pub use config::DataConfig;
pub use error::{ReaderError, Result};
pub use gaps::{Gap, GapSummary};
pub use layout::BucketKey;
pub use load::{LoadedData, load, load_columns};
pub use report::{LoadReport, MissingFile};
pub use types::{PriceType, Symbol, Timeframe};

#[cfg(feature = "python")]
use pyo3::prelude::*;

/// Python extension module (`maturin build` → `import dukascopy_data_reader`).
#[cfg(feature = "python")]
#[pymodule]
fn dukascopy_data_reader(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(python::py_load_columns, module)?)?;
    module.add_function(wrap_pyfunction!(python::py_load, module)?)?;
    Ok(())
}
