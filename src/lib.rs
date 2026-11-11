//! Load Dukascopy OHLCV archive data into column vectors.
//!
//! See the crate README for layout and configuration details.

mod app;
mod config;
mod error;
mod python;
mod types;

pub use app::{KlineColumns, load_columns};
pub use config::DataConfig;
pub use error::{ReaderError, Result};
use pyo3::prelude::*;
pub use types::{Symbol, Timeframe, interval_ms_for, is_supported, supported_timeframes};

/// Python extension module (`maturin build` → `import dukascopy_data_reader`).
#[pymodule]
fn dukascopy_data_reader(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(python::py_load_columns, m)?)?;
    Ok(())
}
