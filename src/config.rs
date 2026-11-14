//! Configuration for one symbol × timeframe × price side.
//!
//! TOML is deserialized into an unvalidated [`RawDataConfig`] and validated
//! exactly once when it becomes a [`DataConfig`]. Generated code and the
//! Python binding use the same constructors, so there is only one validation
//! path.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::Deserialize;

use crate::error::{ReaderError, Result};
use crate::types::{PriceType, Symbol, Timeframe};

/// TOML intermediate representation: strings only, no validation.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDataConfig {
    root: PathBuf,
    symbol: String,
    timeframe: String,
    price_type: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
}

/// Parse an optional `YYYY-MM-DD` field shared by TOML and the Python binding.
pub(crate) fn parse_date(label: &str, value: Option<&str>) -> Result<Option<NaiveDate>> {
    match value {
        None => Ok(None),
        Some(s) => NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
            .map(Some)
            .map_err(|_| ReaderError::Config(format!("invalid {label} (want YYYY-MM-DD): {s:?}"))),
    }
}

/// Validated load request: root, symbol, timeframe, price side and date range.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawDataConfig")]
pub struct DataConfig {
    root: PathBuf,
    symbol: Symbol,
    timeframe: Timeframe,
    price_type: PriceType,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
}

impl DataConfig {
    /// Start a request with the default price side ([`PriceType::Bid`]) and no
    /// date bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ReaderError::Config`] when the symbol is invalid.
    pub fn new(
        root: impl AsRef<Path>,
        symbol: impl AsRef<str>,
        timeframe: Timeframe,
    ) -> Result<Self> {
        Ok(Self {
            root: root.as_ref().to_path_buf(),
            symbol: Symbol::new(symbol)?,
            timeframe,
            price_type: PriceType::Bid,
            start_date: None,
            end_date: None,
        })
    }

    /// Select the quote side.
    #[must_use]
    pub fn with_price_type(mut self, price_type: PriceType) -> Self {
        self.price_type = price_type;
        self
    }

    /// Restrict the request to an inclusive UTC date range.
    ///
    /// # Errors
    ///
    /// Returns [`ReaderError::Config`] when `start_date > end_date`.
    pub fn with_date_range(
        mut self,
        start_date: Option<NaiveDate>,
        end_date: Option<NaiveDate>,
    ) -> Result<Self> {
        if let (Some(start), Some(end)) = (start_date, end_date)
            && start > end
        {
            return Err(ReaderError::Config(format!(
                "start_date {start} is after end_date {end}"
            )));
        }
        self.start_date = start_date;
        self.end_date = end_date;
        Ok(self)
    }

    /// Data root: the downloader's `output_dir` (the reader appends `dukascopy/`).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validated symbol.
    #[must_use]
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }

    /// Candle timeframe.
    #[must_use]
    pub fn timeframe(&self) -> Timeframe {
        self.timeframe
    }

    /// Quote side to read.
    #[must_use]
    pub fn price_type(&self) -> PriceType {
        self.price_type
    }

    /// Inclusive lower date bound (UTC).
    #[must_use]
    pub fn start_date(&self) -> Option<NaiveDate> {
        self.start_date
    }

    /// Inclusive upper date bound (UTC); covers the whole day.
    #[must_use]
    pub fn end_date(&self) -> Option<NaiveDate> {
        self.end_date
    }
}

impl TryFrom<RawDataConfig> for DataConfig {
    type Error = ReaderError;

    fn try_from(raw: RawDataConfig) -> Result<Self> {
        let timeframe: Timeframe = raw.timeframe.parse()?;
        let price_type = match raw.price_type {
            Some(value) => value.parse()?,
            None => PriceType::Bid,
        };
        let start_date = parse_date("start_date", raw.start_date.as_deref())?;
        let end_date = parse_date("end_date", raw.end_date.as_deref())?;
        Self::new(&raw.root, &raw.symbol, timeframe)?
            .with_price_type(price_type)
            .with_date_range(start_date, end_date)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_uses_bid_by_default() {
        let cfg: DataConfig = toml::from_str(
            r#"
root = "/data/data"
symbol = "EURUSD"
timeframe = "D1"
"#,
        )
        .unwrap();
        assert_eq!(cfg.root(), Path::new("/data/data"));
        assert_eq!(cfg.symbol().as_str(), "EURUSD");
        assert_eq!(cfg.timeframe(), Timeframe::D1);
        assert_eq!(cfg.price_type(), PriceType::Bid);
        assert_eq!(cfg.start_date(), None);
    }

    #[test]
    fn toml_accepts_price_type_aliases_and_dates() {
        let cfg: DataConfig = toml::from_str(
            r#"
root = "/a/b c"
symbol = "XAUUSD"
timeframe = "1h"
price_type = "ask"
start_date = "2024-01-01"
end_date = "2024-12-31"
"#,
        )
        .unwrap();
        assert_eq!(cfg.timeframe(), Timeframe::H1);
        assert_eq!(cfg.price_type(), PriceType::Ask);
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
    fn legacy_keys_are_rejected() {
        for legacy in [
            "dir = \"/data\"\nroot = \"/data\"\n",
            "type = \"binance\"\nroot = \"/data\"\n",
        ] {
            let toml_str = format!("{legacy}symbol = \"EURUSD\"\ntimeframe = \"D1\"\n");
            assert!(toml::from_str::<DataConfig>(&toml_str).is_err());
        }
    }

    #[test]
    fn invalid_values_are_rejected() {
        assert!(DataConfig::new("/data", "", Timeframe::D1).is_err());
        assert!(
            DataConfig::new("/data", "EURUSD", Timeframe::D1)
                .unwrap()
                .with_date_range(
                    Some(NaiveDate::from_ymd_opt(2025, 1, 2).unwrap()),
                    Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
                )
                .is_err()
        );
        assert!(parse_date("start_date", Some("2024-13-01")).is_err());
        assert!("4h".parse::<Timeframe>().is_err());
        assert!("mid".parse::<PriceType>().is_err());
    }

    #[test]
    fn builder_defaults_to_bid_and_no_bounds() {
        let cfg = DataConfig::new("/data", "EURUSD", Timeframe::M1).unwrap();
        assert_eq!(cfg.price_type(), PriceType::Bid);
        assert!(cfg.start_date().is_none());
        let cfg = cfg.with_price_type(PriceType::Ask);
        assert_eq!(cfg.price_type(), PriceType::Ask);
    }
}
