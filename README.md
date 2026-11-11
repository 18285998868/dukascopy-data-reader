# dukascopy-data-reader

> 从本地 Dukascopy 公共归档加载 OHLCV 为列向量。只负责加载、读取、清洗数据；
> 格式转换由下游自行处理。

## 数据布局

`dir` 为数据根目录，reader 自动追加 `dukascopy/{symbol}/`：

```text
{dir}/dukascopy/{symbol}/{YYYY}/BID_candles_day_1.bi5
{dir}/dukascopy/{symbol}/{YYYY}/{MM}/BID_candles_hour_1.bi5
{dir}/dukascopy/{symbol}/{YYYY}/{MM}/{DD}/BID_candles_min_1.bi5
```

- LZMA 压缩，每条记录 24 字节大端：
  `time_secs:u32` + `open/close/low/high`（各 u32，`/100000`）+ `volume:u32`。
- 支持 `M1`/`H1`/`D1`，另兼容 `1m`/`1h`/`1d`。
- 同时扫描 `BID_` 与 `ASK_` 文件；周期与请求时间帧不匹配的文件跳过。

## 配置

```toml
dir = "path/to/data"
symbol = "EURUSD"
timeframe = "D1" # M1 | H1 | D1 (兼容 1m | 1h | 1d)
# start_date = "2024-01-01"
# end_date = "2024-12-31" # 含当日整天 (UTC)
```

```rust
let cfg: DataConfig = toml::from_str(&config_str)?;
let cols: KlineColumns = load_columns(&cfg)?;
```

`DataConfig` 构造即完成全部校验（品种、周期、日期范围），非法输入返回
`ReaderError::Config`。

## Python

```python
import dukascopy_data_reader

cols = dukascopy_data_reader.load_columns({
    "dir": "path/to/data",
    "symbol": "EURUSD",
    "timeframe": "D1",
})
```

Python 绑定只负责解析参数并调用 Rust 接口。

## 处理语义

- 单文件硬失败零容忍：损坏 `.bi5`、非法文件名日期 → 整批 `Err`。
- 行级坏 OHLC warn 并跳过（留样上限 5）。
- 年/月文件与请求区间重叠时整文件读取，再按行裁剪到
  `[start 00:00, end 次日 00:00)`。
- 全局按 `open_time` 排序、去重 keep-first、缺口只报告不填充。

## 架构

```text
src/
  lib.rs                 导出 + PyO3 模块
  config.rs              DataConfig
  types.rs               Symbol / Timeframe
  app/mod.rs             load_columns 编排
  app/01_reader/dukascopy.rs  扫描、解压、解析
  app/02_clean.rs        去重与缺口检查
  app/03_column.rs       OHLCV 列转置
```

依赖：`lzma-rs`、`walkdir`、`chrono`、`rayon`、`serde`、`thiserror`、`tracing`、
`pyo3`（abi3-py39）。
