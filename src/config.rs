//! Configuration for one symbol × one timeframe (Dukascopy archives).
//!
//! TOML is first deserialized into an unvalidated intermediate form, then
//! validated exactly once at the [`DataConfig`] boundary.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::Deserialize;

use crate::error::ReaderError;
use crate::types::{Symbol, Timeframe};

/// TOML intermediate representation: strings only, no validation.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDataConfig {
    dir: PathBuf,
    symbol: String,
    timeframe: String,
    start_date: Option<String>,
    end_date: Option<String>,
}

fn parse_opt_date(label: &str, v: Option<&str>) -> Result<Option<NaiveDate>, ReaderError> {
    match v {
        None => Ok(None),
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(Some)
            .map_err(|_| ReaderError::Config(format!("invalid {label} (want YYYY-MM-DD): {s}"))),
    }
}

/// 单品种 × 单时间帧的数据加载配置：构造即有效（`parse-don't-validate`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawDataConfig")]
pub struct DataConfig {
    dir: PathBuf,
    symbol: Symbol,
    timeframe: Timeframe,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
}

impl DataConfig {
    /// 编程构造入口：一次解析全部输入，非法即 `Err`（fail-fast）。
    pub fn new(
        dir: impl AsRef<Path>,
        symbol: &str,
        timeframe: &str,
        start_date: Option<&str>,
        end_date: Option<&str>,
    ) -> Result<Self, ReaderError> {
        let symbol = Symbol::parse(symbol)?;
        let timeframe = Timeframe::parse(timeframe)?;
        let start = parse_opt_date("start_date", start_date)?;
        let end = parse_opt_date("end_date", end_date)?;
        if let (Some(s), Some(e)) = (start, end)
            && s > e
        {
            return Err(ReaderError::Config(format!(
                "start_date {s} is after end_date {e}"
            )));
        }
        Ok(Self {
            dir: dir.as_ref().to_path_buf(),
            symbol,
            timeframe,
            start_date: start,
            end_date: end,
        })
    }

    /// Data root; the reader appends `dukascopy/{symbol}/...`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Borrow the validated symbol.
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }

    /// Borrow the validated timeframe.
    pub fn timeframe(&self) -> &Timeframe {
        &self.timeframe
    }

    /// Inclusive lower date bound (UTC).
    pub fn start_date(&self) -> Option<NaiveDate> {
        self.start_date
    }

    /// Inclusive upper date bound (UTC); covers the whole day.
    pub fn end_date(&self) -> Option<NaiveDate> {
        self.end_date
    }
}

impl TryFrom<RawDataConfig> for DataConfig {
    type Error = ReaderError;

    fn try_from(raw: RawDataConfig) -> Result<Self, Self::Error> {
        Self::new(
            &raw.dir,
            &raw.symbol,
            &raw.timeframe,
            raw.start_date.as_deref(),
            raw.end_date.as_deref(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_roundtrip() {
        let toml_str = r#"
dir = "/data/dukascopy"
symbol = "EURUSD"
timeframe = "D1"
"#;
        let cfg: DataConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.dir(), Path::new("/data/dukascopy"));
        assert_eq!(cfg.symbol().as_str(), "EURUSD");
        assert_eq!(cfg.timeframe().as_str(), "D1");
        assert_eq!(cfg.timeframe().interval_ms(), 86_400_000);
        assert!(cfg.start_date().is_none());
    }

    #[test]
    fn toml_with_dates_and_aliases() {
        let toml_str = r#"
dir = "/a/b c"
symbol = "XAUUSD"
timeframe = "1h"
start_date = "2024-01-01"
end_date = "2024-12-31"
"#;
        let cfg: DataConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.dir(), Path::new("/a/b c"));
        assert_eq!(cfg.timeframe().interval_ms(), 3_600_000);
        assert_eq!(
            cfg.start_date(),
            Some(NaiveDate::from_ymd_opt(2024, 1, 1).unwrap())
        );
        assert_eq!(
            cfg.end_date(),
            Some(NaiveDate::from_ymd_opt(2024, 12, 31).unwrap())
        );
    }

    #[test]
    fn legacy_source_keys_are_rejected() {
        for extra in ["type = \"binance\"\n", "market = \"spot\"\n"] {
            let toml_str =
                format!("{extra}dir = \"/data\"\nsymbol = \"EURUSD\"\ntimeframe = \"D1\"\n");
            assert!(toml::from_str::<DataConfig>(&toml_str).is_err());
        }
    }

    #[test]
    fn invalid_inputs_are_rejected() {
        assert!(DataConfig::new("/data", "", "D1", None, None).is_err());
        assert!(DataConfig::new("/data", "EURUSD", "4h", None, None).is_err());
        assert!(DataConfig::new("/data", "EURUSD", "D1", Some("2024-13-01"), None).is_err());
        assert!(
            DataConfig::new(
                "/data",
                "EURUSD",
                "D1",
                Some("2025-01-02"),
                Some("2025-01-01")
            )
            .is_err()
        );
    }
}
