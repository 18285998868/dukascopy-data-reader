use crate::app::Record;

/// Column-oriented OHLCV output.
#[derive(Debug, Clone)]
pub struct KlineColumns {
    time: Vec<i64>,
    open: Vec<f64>,
    high: Vec<f64>,
    low: Vec<f64>,
    close: Vec<f64>,
    volume: Vec<f64>,
}

impl KlineColumns {
    /// Construct validated, equal-length columns.
    pub fn new(
        time: Vec<i64>,
        open: Vec<f64>,
        high: Vec<f64>,
        low: Vec<f64>,
        close: Vec<f64>,
        volume: Vec<f64>,
    ) -> crate::error::Result<Self> {
        if time.len() != open.len()
            || time.len() != high.len()
            || time.len() != low.len()
            || time.len() != close.len()
            || time.len() != volume.len()
        {
            return Err(crate::error::ReaderError::Config(
                "kline columns must have equal length".into(),
            ));
        }
        Ok(Self {
            time,
            open,
            high,
            low,
            close,
            volume,
        })
    }

    /// Bar open times in milliseconds.
    pub fn time(&self) -> &[i64] {
        &self.time
    }

    /// Open prices.
    pub fn open(&self) -> &[f64] {
        &self.open
    }

    /// High prices.
    pub fn high(&self) -> &[f64] {
        &self.high
    }

    /// Low prices.
    pub fn low(&self) -> &[f64] {
        &self.low
    }

    /// Close prices.
    pub fn close(&self) -> &[f64] {
        &self.close
    }

    /// Volumes.
    pub fn volume(&self) -> &[f64] {
        &self.volume
    }

    /// Consume the column set, returning the raw vectors in column order.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        Vec<i64>,
        Vec<f64>,
        Vec<f64>,
        Vec<f64>,
        Vec<f64>,
        Vec<f64>,
    ) {
        (
            self.time,
            self.open,
            self.high,
            self.low,
            self.close,
            self.volume,
        )
    }
}

impl KlineColumns {
    #[must_use]
    pub const fn len(&self) -> usize {
        self.close.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.close.is_empty()
    }
}

impl From<Vec<Record>> for KlineColumns {
    fn from(records: Vec<Record>) -> Self {
        let n = records.len();
        let mut cols = KlineColumns {
            time: Vec::with_capacity(n),
            open: Vec::with_capacity(n),
            high: Vec::with_capacity(n),
            low: Vec::with_capacity(n),
            close: Vec::with_capacity(n),
            volume: Vec::with_capacity(n),
        };
        for r in records {
            cols.time.push(r.open_time);
            cols.open.push(r.open);
            cols.high.push(r.high);
            cols.low.push(r.low);
            cols.close.push(r.close);
            cols.volume.push(r.volume);
        }
        cols
    }
}
