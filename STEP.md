# 执行步骤

## 模块一：配置

1. TOML 反序列化为 `RawDataConfig`，拒绝未知字段。
2. `TryFrom` 校验 symbol、timeframe、price side 与日期顺序。
3. 生成字段私有的 `DataConfig`；非法值返回 `ReaderError::Config`。
4. `root` 保持 downloader 的 `output_dir` 语义，不追加子路径。

```bash
cargo test config::types
```

## 模块二：文件计划

1. `symbol_dir` 追加 `dukascopy/{symbol}/`，并确认它是目录。
2. 两端日期都有：`buckets_between` 直接展开，不读目录树。
3. 无日期：枚举磁盘上实际存在的桶。
4. 单边日期：只枚举一次磁盘，用另一端补齐范围。
5. `BucketKey` 保存逻辑日历日期；渲染路径时月份减一，日为 1-based。
6. 对每个桶生成 `{year}/[{month}/[{day}/]]{BID|ASK}_candles_*.bi5`。

```bash
cargo test layout:: load::tests::fully_bounded
```

## 模块三：解码

1. 逐文件检查存在性；缺失进入报告，不算错误。
2. 每 64 个文件一批，rayon 并行打开、LZMA 解压、按 24 字节切分。
3. LZMA 失败或长度非 24 倍数 → `ReaderError::CorruptFile { path, .. }`。
4. 记录转为 `time_ms = 桶起点 + time_secs * 1000`，价格除以 100000。
5. 时间戳不在桶内、OHLC 非法 → 跳过并计数。
6. 每批解码完立即追加到列缓冲，控制峰值内存。

```bash
cargo test decode:: load::tests::corrupt
```

## 模块四：合并与清洗

1. 全部批次按请求顺序追加。
2. 有日期边界时按 `[start 00:00, end 次日 00:00)` 裁剪并计数。
3. 仅当时间列乱序时做稳定排序。
4. 相邻重复 `open_time` 保留首行并计数。
5. 扫描缺口，记录段数、缺失 bar 数、错位段与最多 8 个样本。
6. 生成 `LoadReport`；`is_complete()` 要求无缺失、无异常行、无重复、无缺口。

```bash
cargo test columns:: gaps:: report::
```

## 模块五：对外接口

1. Rust：`DataConfig`、`load`、`load_columns`、`LoadedData`、
   `KlineColumns`、`LoadReport`。
2. Python（`python` feature）：字典配置 → `DataConfig`；返回 NumPy 数组。
3. 加载在 `Python::detach` 中执行，不持有 GIL。

```bash
cargo test --features python
```

## 模块六：验收

1. 离线门禁：格式、check、clippy、test、doctest、doc、三档 feature。
2. 真实冒烟：用 `config/00.smoke.toml` 加载真实 `.bi5`，退出码必须为 0。
3. 二次运行：保留数据再跑一次，结果与退出码不变。

```bash
./scripts/check.sh
cargo run --release --example load -- config/00.smoke.toml
cargo run --release --example load -- config/00.smoke.toml
```
