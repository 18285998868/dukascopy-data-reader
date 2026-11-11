//! Domain newtypes: symbol and Dukascopy timeframe.

use std::fmt;

use crate::error::ReaderError;

/// Single source of truth: timeframe → expected bar interval in milliseconds.
const TIMEFRAME_INTERVALS_MS: &[(&str, i64)] = &[
    ("M1", 60_000),
    ("1m", 60_000),
    ("H1", 3_600_000),
    ("1h", 3_600_000),
    ("D1", 86_400_000),
    ("1d", 86_400_000),
];

/// Dukascopy timeframes accepted by [`Timeframe`].
const DUKASCOPY_TIMEFRAMES: &[&str] = &["M1", "H1", "D1", "1m", "1h", "1d"];

/// Interval lookup: unknown timeframes return `None`.
#[must_use]
pub fn interval_ms_for(timeframe: &str) -> Option<i64> {
    TIMEFRAME_INTERVALS_MS
        .iter()
        .find(|(tf, _)| *tf == timeframe)
        .map(|(_, ms)| *ms)
}

/// Whether `timeframe` is supported by Dukascopy archives.
#[must_use]
pub fn is_supported(timeframe: &str) -> bool {
    DUKASCOPY_TIMEFRAMES.contains(&timeframe)
}

/// List of supported Dukascopy timeframes.
#[must_use]
pub fn supported_timeframes() -> &'static [&'static str] {
    DUKASCOPY_TIMEFRAMES
}

/// 已验证的交易对符号：构造时保证非空（去首尾空白后）。
/// 后续函数只收 `&Symbol`，不再重复判空（`api-parse-dont-validate`）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Symbol(String);

impl Symbol {
    /// Parse and validate a symbol (non-empty after trimming).
    pub fn parse(s: impl AsRef<str>) -> Result<Self, ReaderError> {
        let trimmed = s.as_ref().trim();
        if trimmed.is_empty() {
            return Err(ReaderError::Config("symbol must not be empty".into()));
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// Borrow the inner symbol string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Symbol {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// 已验证的时间周期：构造时按 Dukascopy 白名单校验，并绑定间隔。
/// `interval_ms` 在构造时即确定，`clean` 不再二次查表失败。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Timeframe {
    raw: String,
    interval_ms: i64,
}

impl Timeframe {
    /// Parse and validate a Dukascopy timeframe.
    pub fn parse(s: impl AsRef<str>) -> Result<Self, ReaderError> {
        let s = s.as_ref().trim();
        if !is_supported(s) {
            return Err(ReaderError::Config(format!(
                "unsupported dukascopy timeframe: {s}, valid values: {:?}",
                supported_timeframes()
            )));
        }
        let interval_ms = interval_ms_for(s).ok_or_else(|| {
            ReaderError::Config(format!("no interval defined for timeframe: {s}"))
        })?;
        Ok(Self {
            raw: s.to_owned(),
            interval_ms,
        })
    }

    /// Borrow the raw timeframe string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Configured bar interval in milliseconds.
    #[must_use]
    pub const fn interval_ms(&self) -> i64 {
        self.interval_ms
    }
}

impl fmt::Display for Timeframe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl AsRef<str> for Timeframe {
    fn as_ref(&self) -> &str {
        &self.raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_rejects_empty() {
        assert!(Symbol::parse("").is_err());
        assert!(Symbol::parse("   ").is_err());
        assert_eq!(Symbol::parse(" EURUSD ").unwrap().as_str(), "EURUSD");
    }

    #[test]
    fn timeframe_accepts_dukascopy_whitelist() {
        assert!(Timeframe::parse("M1").is_ok());
        assert!(Timeframe::parse("D1").is_ok());
        assert_eq!(Timeframe::parse(" 1h ").unwrap().as_str(), "1h");
        assert!(Timeframe::parse("4h").is_err());
        assert!(Timeframe::parse("1w").is_err());
    }

    #[test]
    fn timeframe_binds_interval() {
        assert_eq!(Timeframe::parse("M1").unwrap().interval_ms(), 60_000);
        assert_eq!(Timeframe::parse("H1").unwrap().interval_ms(), 3_600_000);
        assert_eq!(Timeframe::parse("D1").unwrap().interval_ms(), 86_400_000);
        assert_eq!(interval_ms_for("1d"), Some(86_400_000));
        assert_eq!(interval_ms_for("3d"), None);
        assert!(supported_timeframes().contains(&"H1"));
    }
}
