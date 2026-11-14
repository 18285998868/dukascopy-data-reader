//! Domain types: symbol, timeframe and price side.
//!
//! All three validate at construction time, so the loader only accepts values
//! that map to a known Dukascopy archive layout.

use std::fmt;
use std::str::FromStr;

use crate::error::ReaderError;

/// Dukascopy candle timeframe.
///
/// The archive stores one file per calendar bucket: `M1` per day, `H1` per
/// month, `D1` per year.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Timeframe {
    /// One-minute candles, bucketed by UTC day.
    M1,
    /// One-hour candles, bucketed by UTC month.
    H1,
    /// One-day candles, bucketed by UTC year.
    D1,
}

impl Timeframe {
    /// Every supported timeframe.
    pub const ALL: [Self; 3] = [Self::M1, Self::H1, Self::D1];

    /// Canonical name (`M1`, `H1`, `D1`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::M1 => "M1",
            Self::H1 => "H1",
            Self::D1 => "D1",
        }
    }

    /// Bar interval in milliseconds.
    #[must_use]
    pub const fn interval_ms(self) -> i64 {
        match self {
            Self::M1 => 60_000,
            Self::H1 => 3_600_000,
            Self::D1 => 86_400_000,
        }
    }

    /// Filename part used by the archive (`candles_min_1`, ...).
    #[must_use]
    pub const fn file_stem(self) -> &'static str {
        match self {
            Self::M1 => "candles_min_1",
            Self::H1 => "candles_hour_1",
            Self::D1 => "candles_day_1",
        }
    }
}

impl fmt::Display for Timeframe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Timeframe {
    type Err = ReaderError;

    /// Accepts the canonical names and the `1m` / `1h` / `1d` aliases.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let raw = s.trim();
        match raw.to_ascii_lowercase().as_str() {
            "m1" | "1m" => Ok(Self::M1),
            "h1" | "1h" => Ok(Self::H1),
            "d1" | "1d" => Ok(Self::D1),
            _ => Err(ReaderError::Config(format!(
                "unsupported dukascopy timeframe: {raw:?} (valid: M1, H1, D1; aliases: 1m, 1h, 1d)"
            ))),
        }
    }
}

/// Side of the quote a candle series describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PriceType {
    /// Bid-side candles (`BID_*.bi5`).
    Bid,
    /// Ask-side candles (`ASK_*.bi5`).
    Ask,
}

impl PriceType {
    /// Every supported price side.
    pub const ALL: [Self; 2] = [Self::Bid, Self::Ask];

    /// Uppercase filename prefix used by the archive.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Bid => "BID",
            Self::Ask => "ASK",
        }
    }

    /// Canonical lowercase name (`bid`, `ask`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bid => "bid",
            Self::Ask => "ask",
        }
    }
}

impl fmt::Display for PriceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PriceType {
    type Err = ReaderError;

    /// Accepts `BID` / `ASK` in any ASCII case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "bid" => Ok(Self::Bid),
            "ask" => Ok(Self::Ask),
            _ => Err(ReaderError::Config(format!(
                "unsupported price type: {s:?} (valid: BID, ASK)"
            ))),
        }
    }
}

/// Validated Dukascopy symbol (the directory name below `dukascopy/`).
///
/// Only ASCII alphanumerics plus `_`, `-` and `.` are accepted, and `.` / `..`
/// are rejected, so a symbol can never escape its data directory.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Symbol(String);

impl Symbol {
    /// Parse and validate a symbol.
    ///
    /// # Errors
    ///
    /// Returns [`ReaderError::Config`] when the trimmed value is empty or
    /// contains characters outside `[A-Za-z0-9._-]`.
    pub fn new(s: impl AsRef<str>) -> Result<Self, ReaderError> {
        let trimmed = s.as_ref().trim();
        if trimmed.is_empty() {
            return Err(ReaderError::Config("symbol must not be empty".into()));
        }
        let valid = trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
        if !valid || trimmed == "." || trimmed == ".." {
            return Err(ReaderError::Config(format!(
                "invalid symbol: {trimmed:?} (allowed: ASCII letters, digits, '_', '-', '.')"
            )));
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// Borrow the symbol string.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_rejects_empty() {
        assert!(Symbol::new("").is_err());
        assert!(Symbol::new("   ").is_err());
        assert_eq!(Symbol::new(" EURUSD ").unwrap().as_str(), "EURUSD");
    }

    #[test]
    fn symbol_rejects_path_traversal() {
        for bad in ["..", ".", "../X", "A/B", "A\\B", "EUR USD"] {
            assert!(Symbol::new(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(Symbol::new("XAUUSD").is_ok());
        assert!(Symbol::new("EURUSD_otc").is_ok());
    }

    #[test]
    fn timeframe_accepts_canonical_names_and_aliases() {
        for (input, expected) in [
            ("M1", Timeframe::M1),
            ("m1", Timeframe::M1),
            ("1m", Timeframe::M1),
            ("H1", Timeframe::H1),
            ("1h", Timeframe::H1),
            (" D1 ", Timeframe::D1),
            ("1d", Timeframe::D1),
        ] {
            assert_eq!(input.parse::<Timeframe>().unwrap(), expected, "{input}");
        }
        assert!("4h".parse::<Timeframe>().is_err());
        assert!("".parse::<Timeframe>().is_err());
    }

    #[test]
    fn timeframe_maps_to_layout_and_interval() {
        assert_eq!(Timeframe::M1.file_stem(), "candles_min_1");
        assert_eq!(Timeframe::H1.file_stem(), "candles_hour_1");
        assert_eq!(Timeframe::D1.file_stem(), "candles_day_1");
        assert_eq!(Timeframe::M1.interval_ms(), 60_000);
        assert_eq!(Timeframe::H1.interval_ms(), 3_600_000);
        assert_eq!(Timeframe::D1.interval_ms(), 86_400_000);
        assert_eq!(Timeframe::D1.to_string(), "D1");
    }

    #[test]
    fn price_type_parses_case_insensitively() {
        assert_eq!("bid".parse::<PriceType>().unwrap(), PriceType::Bid);
        assert_eq!("ASK".parse::<PriceType>().unwrap(), PriceType::Ask);
        assert_eq!(PriceType::Bid.prefix(), "BID");
        assert_eq!(PriceType::Ask.prefix(), "ASK");
        assert!("mid".parse::<PriceType>().is_err());
    }
}
