//! Python bindings (`pyo3`, abi3): thin `load_columns(dict) -> dict` wrapper.
//!
//! The dict mirrors `DataConfig` (`dir/symbol/timeframe/start_date/end_date`)
//! and returns `{time, open, high, low, close, volume}` column lists.
//! 解析在边界一次完成（`DataConfig::new`），非法输入直接报 `ValueError`。

use pyo3::exceptions::{PyKeyError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::config::DataConfig;
use crate::error::ReaderError;

fn py_err(e: ReaderError) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
}

fn extract_required<'py, T>(cfg: &Bound<'py, PyDict>, key: &str) -> PyResult<T>
where
    T: for<'a> FromPyObject<'a, 'py>,
{
    cfg.get_item(key)?
        .ok_or_else(|| PyKeyError::new_err(format!("missing required config key: {key}")))?
        .extract()
        .map_err(|_| PyTypeError::new_err(format!("config[{key}] has wrong type")))
}

fn extract_optional<'py, T>(cfg: &Bound<'py, PyDict>, key: &str) -> PyResult<Option<T>>
where
    T: for<'a> FromPyObject<'a, 'py>,
{
    cfg.get_item(key)?
        .map(|v| {
            v.extract()
                .map_err(|_| PyTypeError::new_err(format!("config[{key}] has wrong type")))
        })
        .transpose()
}

/// Load one symbol × one timeframe of Dukascopy candles as OHLCV columns.
#[pyfunction]
#[pyo3(name = "load_columns", signature = (config))]
pub(crate) fn py_load_columns(config: &Bound<'_, PyDict>) -> PyResult<Py<PyAny>> {
    let dir: String = extract_required(config, "dir")?;
    let symbol: String = extract_required(config, "symbol")?;
    let timeframe: String = extract_required(config, "timeframe")?;
    let start_date: Option<String> = extract_optional(config, "start_date")?;
    let end_date: Option<String> = extract_optional(config, "end_date")?;
    let data_cfg = DataConfig::new(
        &dir,
        &symbol,
        &timeframe,
        start_date.as_deref(),
        end_date.as_deref(),
    )
    .map_err(|e| PyValueError::new_err(e.to_string()))?;
    let cols = crate::app::load_columns(&data_cfg).map_err(py_err)?;
    Python::attach(|py| {
        let out = PyDict::new(py);
        out.set_item("time", cols.time())?;
        out.set_item("open", cols.open())?;
        out.set_item("high", cols.high())?;
        out.set_item("low", cols.low())?;
        out.set_item("close", cols.close())?;
        out.set_item("volume", cols.volume())?;
        Ok(out.into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyDict;

    #[test]
    fn py_load_columns_missing_key_errors() {
        Python::initialize();
        Python::attach(|py| {
            let cfg = PyDict::new(py);
            let err = py_load_columns(&cfg).unwrap_err();
            assert!(err.to_string().contains("missing required config key"));
        });
    }

    #[test]
    fn py_load_columns_invalid_timeframe_errors() {
        Python::initialize();
        Python::attach(|py| {
            let cfg = PyDict::new(py);
            cfg.set_item("dir", "/tmp").unwrap();
            cfg.set_item("symbol", "EURUSD").unwrap();
            cfg.set_item("timeframe", "4h").unwrap();
            let err = py_load_columns(&cfg).unwrap_err();
            assert!(err.to_string().contains("unsupported dukascopy timeframe"));
        });
    }
}
