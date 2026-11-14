# dukascopy-data-reader

从本地 Dukascopy 公共归档读取 OHLCV 列向量。只负责加载、解码与报告；
格式转换、指标计算由下游完成。

## 数据布局

`root` 是 downloader 的 `output_dir`，reader 自动追加 `dukascopy/{symbol}/`：

```text
{root}/dukascopy/{symbol}/{YYYY}/BID_candles_day_1.bi5
{root}/dukascopy/{symbol}/{YYYY}/{MM}/BID_candles_hour_1.bi5
{root}/dukascopy/{symbol}/{YYYY}/{MM}/{DD}/BID_candles_min_1.bi5
```

- 路径月份为 **0-based**：`00` = 1 月，`11` = 12 月；年、日仍为 1-based。
  这是 Dukascopy datafeed 的原始约定，本地镜像与 URL 完全一致。
- `D1` 每年一个文件，`H1` 每月一个，`M1` 每天一个。
- LZMA 压缩，每条记录 24 字节大端：
  `time_secs:u32, open:u32, close:u32, low:u32, high:u32, volume:u32`。
  价格除以 `100000`，时间戳是相对桶起点的秒偏移。
- 支持 `M1`/`H1`/`D1`，别名 `1m`/`1h`/`1d`；价格侧 `BID`/`ASK`。

## Rust

```toml
[dependencies]
dukascopy-data-reader = "0.2"
```

```rust
use chrono::NaiveDate;
use dukascopy_data_reader::{DataConfig, Timeframe, load};

let config = DataConfig::new("/data/data", "EURUSD", Timeframe::H1)?
    .with_date_range(
        Some(NaiveDate::from_ymd_opt(2023, 7, 1).unwrap()),
        Some(NaiveDate::from_ymd_opt(2023, 7, 31).unwrap()),
    )?;
let loaded = load(&config)?;
assert_eq!(loaded.columns().len(), 744);
assert!(loaded.report().is_complete());
```

`load_columns` 只返回列；`load` 同时返回 `LoadReport`：

- 请求/加载/缺失文件数，缺失路径与对应桶；
- 解码、裁剪、非法 OHLC、越界时间戳、重复行计数；
- 缺口段数、缺失 bar 数、错位段数与有限样本；
- 首末 bar 时间与 `is_complete()`。

列均为等长 `Vec`：`time: i64`（Unix 毫秒，UTC）、OHLC `f64`、`volume: u32`
（原始值，不归一化）。

## 配置

```toml
root = "/data/data"
symbol = "EURUSD"
timeframe = "H1"
price_type = "BID" # 可选，默认 BID
start_date = "2023-07-01" # 可选，含当日 (UTC)
end_date = "2023-07-31" # 可选，含当日 (UTC)
```

`DataConfig` 在构造/反序列化时完成全部校验，未知字段直接报错。
完整字段见 [`config/00.example.toml`](config/00.example.toml)。

## Python

```bash
python -m pip install maturin
maturin develop --features python
```

```python
import dukascopy_data_reader

columns = dukascopy_data_reader.load_columns({
    "root": "/data/data",
    "symbol": "EURUSD",
    "timeframe": "H1",
    "start_date": "2023-07-01",
    "end_date": "2023-07-31",
})
# time int64[ms], OHLC float64, volume uint32
print(columns["time"][:3], columns["close"][-1])
```

`load()` 返回 `(columns, report)`。Python 绑定与 Rust 共用同一套校验和加载路径。

## 处理语义

- 损坏 `.bi5`（LZMA 失败、解压长度非 24 倍数）→ 带路径的硬错误。
- 缺失文件不报错，进入 `LoadReport::missing_files`；`is_complete()` 为 false。
- 非法 OHLC 行跳过并计数；时间戳不在所属桶内的行跳过并计数。
- 有界区间只展开请求的桶，不扫描全历史；解码按 64 文件分批并行。
- 仅当时间列乱序时排序；重复 `open_time` 保留首行并计数。
- 缺口只报告、不填充；Dukascopy 的周末/休市零量平坦 bar 原样保留。

## 开发

```bash
./scripts/check.sh
cargo run --release --example load -- config/00.smoke.toml
```

执行路径与退出码见 [STEP.md](STEP.md)，版本变化见 [CHANGELOG.md](CHANGELOG.md)。

## 许可证

MIT
