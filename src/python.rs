//! Python bindings: `load_columns(config)` and `load(config)`.
//!
//! Both accept the same dict:
//!
//! ```python
//! {
//!     "root": "/data/data",          # downloader output_dir
//!     "symbol": "EURUSD",
//!     "timeframe": "H1",             # M1 | H1 | D1 (1m/1h/1d accepted)
//!     "price_type": "BID",           # optional, default BID
//!     "start_date": "2023-06-01",    # optional, inclusive UTC
//!     "end_date": "2023-06-30",      # optional, inclusive UTC
//! }
//! ```
//!
//! `load_columns` returns `{time, open, high, low, close, volume}` NumPy
//! arrays (`int64` and `float64`, volume `uint32`). `load` returns
//! `(columns, report)`.

use numpy::IntoPyArray;
use pyo3::exceptions::{PyKeyError, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::columns::KlineColumns;
use crate::config::{DataConfig, parse_date};
use crate::error::ReaderError;
use crate::report::LoadReport;
use crate::types::{PriceType, Timeframe};

fn extract_required<'py, T>(config: &Bound<'py, PyDict>, key: &str) -> PyResult<T>
where
    T: for<'a> FromPyObject<'a, 'py>,
{
    config
        .get_item(key)?
        .ok_or_else(|| PyKeyError::new_err(format!("missing required config key: {key}")))?
        .extract()
        .map_err(|_| PyTypeError::new_err(format!("config[{key}] has wrong type")))
}

fn extract_optional<'py, T>(config: &Bound<'py, PyDict>, key: &str) -> PyResult<Option<T>>
where
    T: for<'a> FromPyObject<'a, 'py>,
{
    match config.get_item(key)? {
        None => Ok(None),
        Some(value) if value.is_none() => Ok(None),
        Some(value) => value
            .extract()
            .map(Some)
            .map_err(|_| PyTypeError::new_err(format!("config[{key}] has wrong type"))),
    }
}

fn config_from_dict(config: &Bound<'_, PyDict>) -> PyResult<DataConfig> {
    let root: String = extract_required(config, "root")?;
    let symbol: String = extract_required(config, "symbol")?;
    let timeframe: String = extract_required(config, "timeframe")?;
    let price_type: Option<String> = extract_optional(config, "price_type")?;
    let start_date: Option<String> = extract_optional(config, "start_date")?;
    let end_date: Option<String> = extract_optional(config, "end_date")?;

    let timeframe = timeframe.parse::<Timeframe>().map_err(config_error)?;
    let price_type = match price_type {
        Some(value) => value.parse::<PriceType>().map_err(config_error)?,
        None => PriceType::Bid,
    };
    let start_date = parse_date("start_date", start_date.as_deref()).map_err(config_error)?;
    let end_date = parse_date("end_date", end_date.as_deref()).map_err(config_error)?;

    DataConfig::new(&root, &symbol, timeframe)
        .and_then(|config| {
            config
                .with_price_type(price_type)
                .with_date_range(start_date, end_date)
        })
        .map_err(config_error)
}

fn config_error(error: ReaderError) -> PyErr {
    PyValueError::new_err(error.to_string())
}

fn load_error(error: ReaderError) -> PyErr {
    match error {
        ReaderError::Config(_) => PyValueError::new_err(error.to_string()),
        _ => PyRuntimeError::new_err(error.to_string()),
    }
}

fn columns_to_dict<'py>(py: Python<'py>, columns: KlineColumns) -> PyResult<Bound<'py, PyDict>> {
    let parts = columns.into_parts();
    let out = PyDict::new(py);
    out.set_item("time", parts.time.into_pyarray(py))?;
    out.set_item("open", parts.open.into_pyarray(py))?;
    out.set_item("high", parts.high.into_pyarray(py))?;
    out.set_item("low", parts.low.into_pyarray(py))?;
    out.set_item("close", parts.close.into_pyarray(py))?;
    out.set_item("volume", parts.volume.into_pyarray(py))?;
    Ok(out)
}

fn report_to_dict<'py>(py: Python<'py>, report: &LoadReport) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new(py);
    out.set_item("requested_files", report.requested_files())?;
    out.set_item("loaded_files", report.loaded_files())?;
    out.set_item(
        "missing_files",
        report
            .missing_files()
            .iter()
            .map(|missing| format!("{}: {}", missing.bucket(), missing.path().display()))
            .collect::<Vec<_>>(),
    )?;
    out.set_item("rows_decoded", report.rows_decoded())?;
    out.set_item("rows_returned", report.rows_returned())?;
    out.set_item("rows_trimmed", report.rows_trimmed())?;
    out.set_item("rows_invalid", report.rows_invalid())?;
    out.set_item("rows_out_of_bucket", report.rows_out_of_bucket())?;
    out.set_item("duplicates_removed", report.duplicates_removed())?;
    out.set_item("first_bar_ms", report.first_bar_ms())?;
    out.set_item("last_bar_ms", report.last_bar_ms())?;
    out.set_item("complete", report.is_complete())?;

    let gaps = PyDict::new(py);
    gaps.set_item("segments", report.gaps().segments())?;
    gaps.set_item("missing_bars", report.gaps().missing_bars())?;
    gaps.set_item("misaligned_segments", report.gaps().misaligned_segments())?;
    out.set_item("gaps", gaps)?;
    Ok(out)
}

/// Load one symbol × timeframe × price side as NumPy columns.
#[pyfunction]
#[pyo3(name = "load_columns", signature = (config))]
pub(crate) fn py_load_columns<'py>(
    py: Python<'py>,
    config: &Bound<'py, PyDict>,
) -> PyResult<Bound<'py, PyDict>> {
    let config = config_from_dict(config)?;
    let columns = py
        .detach(|| crate::load_columns(&config))
        .map_err(load_error)?;
    columns_to_dict(py, columns)
}

/// Load columns plus the report (`columns, report` tuple).
#[pyfunction]
#[pyo3(name = "load", signature = (config))]
pub(crate) fn py_load<'py>(
    py: Python<'py>,
    config: &Bound<'py, PyDict>,
) -> PyResult<(Bound<'py, PyDict>, Bound<'py, PyDict>)> {
    let config = config_from_dict(config)?;
    let loaded = py.detach(|| crate::load(&config)).map_err(load_error)?;
    let (columns, report) = loaded.into_parts();
    Ok((columns_to_dict(py, columns)?, report_to_dict(py, &report)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::columns::ColumnParts;

    #[test]
    fn missing_key_is_a_key_error() {
        Python::initialize();
        Python::attach(|py| {
            let config = PyDict::new(py);
            let error = py_load_columns(py, &config).unwrap_err();
            assert!(error.is_instance_of::<PyKeyError>(py));
        });
    }

    #[test]
    fn invalid_timeframe_is_a_value_error() {
        Python::initialize();
        Python::attach(|py| {
            let config = PyDict::new(py);
            config.set_item("root", "/tmp").unwrap();
            config.set_item("symbol", "EURUSD").unwrap();
            config.set_item("timeframe", "4h").unwrap();
            let error = py_load_columns(py, &config).unwrap_err();
            assert!(error.is_instance_of::<PyValueError>(py));
            assert!(
                error
                    .to_string()
                    .contains("unsupported dukascopy timeframe")
            );
        });
    }

    #[test]
    fn optional_none_is_accepted() {
        Python::initialize();
        Python::attach(|py| {
            let config = PyDict::new(py);
            config.set_item("root", "/tmp").unwrap();
            config.set_item("symbol", "EURUSD").unwrap();
            config.set_item("timeframe", "D1").unwrap();
            config.set_item("price_type", py.None()).unwrap();
            config.set_item("start_date", py.None()).unwrap();
            let parsed = config_from_dict(&config).unwrap();
            assert_eq!(parsed.price_type(), PriceType::Bid);
            assert!(parsed.start_date().is_none());
        });
    }

    #[test]
    fn numpy_conversion_uses_arrays() {
        Python::initialize();
        Python::attach(|py| {
            let columns = KlineColumns::from_parts(ColumnParts {
                time: vec![1, 2],
                open: vec![1.0, 2.0],
                high: vec![3.0, 4.0],
                low: vec![0.5, 1.5],
                close: vec![2.0, 3.0],
                volume: vec![10, 20],
            })
            .unwrap();
            let dict = columns_to_dict(py, columns).unwrap();
            for key in ["time", "open", "high", "low", "close", "volume"] {
                let array = dict.get_item(key).unwrap().unwrap();
                assert_eq!(array.get_type().name().unwrap(), "ndarray", "{key}");
            }
        });
    }

    #[test]
    fn py_load_columns_reads_the_fixture() {
        Python::initialize();
        Python::attach(|py| {
            let config = PyDict::new(py);
            let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
            config.set_item("root", root).unwrap();
            config.set_item("symbol", "EURUSD").unwrap();
            config.set_item("timeframe", "D1").unwrap();
            let dict = py_load_columns(py, &config).unwrap();
            let time = dict.get_item("time").unwrap().unwrap();
            assert_eq!(time.len().unwrap(), 365);
        });
    }
}
