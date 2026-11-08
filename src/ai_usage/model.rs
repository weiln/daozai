//! 用量数据模型：TokenEntry、日期范围、按 (工具, 模型) 聚合、成本估算。

/// 各工具复用额度页的 Provider 枚举，让「额度」与「用量」在同一 provider 下并列。
pub use crate::quota::Provider;

/// 一条 token 用量记录。各 token 类别互斥；reasoning 已含在 output 内。
#[derive(Clone, Debug, PartialEq)]
pub struct TokenEntry {
    /// 去重键（跨文件同一消息只算一次）。
    pub key: String,
    /// Unix 毫秒。
    pub time: i64,
    pub provider: Provider,
    pub model: String,
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cache_write_1h: u64,
    pub output: u64,
    /// 源自带的成本（USD）；有则直接采用，不再用价格表估算。
    pub reported_cost_usd: Option<f64>,
}

impl TokenEntry {
    pub fn total(&self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.cache_write_1h + self.output
    }
}

/// 本地日历日期范围（含首末整天）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsageDateRange {
    /// 本地日 00:00 的 Unix 毫秒（含）。
    pub start_ms: i64,
    /// 本地日 23:59:59.999 的 Unix 毫秒（含）。
    pub end_ms: i64,
}

impl UsageDateRange {
    pub fn contains(&self, time_ms: i64) -> bool {
        time_ms >= self.start_ms && time_ms <= self.end_ms
    }
    /// 从“今日”起回溯 days 天（1/7/30）。
    pub fn recent(days: i64, now_ms: i64) -> Self {
        let today_start = local_day_start_ms(now_ms);
        let start_ms = today_start - (days - 1) * 86_400_000;
        let end_ms = today_start + 86_400_000 - 1;
        Self { start_ms, end_ms }
    }
}

/// 把 Unix 毫秒对齐到本地当天 00:00（用 chrono 取本地时区偏移）。
pub fn local_day_start_ms(now_ms: i64) -> i64 {
    use chrono::{Local, TimeZone};
    let Some(dt) = Local.timestamp_millis_opt(now_ms).single() else { return now_ms };
    let start = dt.date_naive().and_hms_opt(0, 0, 0).unwrap();
    Local.from_local_datetime(&start).single().map(|d| d.timestamp_millis()).unwrap_or(now_ms)
}

/// 按 (provider, model) 聚合出的一行。
#[derive(Clone, Debug, PartialEq)]
pub struct ModelUsageRow {
    pub provider: Provider,
    pub model: String,
    pub entries: Vec<TokenEntry>,
}

impl ModelUsageRow {
    pub fn sum(&self) -> TokenSums {
        let mut s = TokenSums::default();
        for e in &self.entries {
            s.add(e);
        }
        s
    }
}

/// token 各类别合计。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TokenSums {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cache_write_1h: u64,
    pub output: u64,
    pub total: u64,
}
impl TokenSums {
    fn add(&mut self, e: &TokenEntry) {
        self.input += e.input;
        self.cache_read += e.cache_read;
        self.cache_write += e.cache_write;
        self.cache_write_1h += e.cache_write_1h;
        self.output += e.output;
        self.total += e.total();
    }
}

/// 聚合结果。
#[derive(Clone, Debug, Default)]
pub struct ModelUsage {
    pub rows: Vec<ModelUsageRow>,
    pub sums: TokenSums,
    /// 数据不完整的工具（未读到 / 起始日落后于范围）。
    pub incomplete: Vec<Provider>,
}

/// 聚合：按 (provider, model) 分组，按 total 降序。
pub fn aggregate(entries: &[TokenEntry], selected: &[Provider], range: UsageDateRange) -> ModelUsage {
    use std::collections::HashMap;
    let mut groups: HashMap<(Provider, String), Vec<TokenEntry>> = HashMap::new();
    for e in entries {
        if !selected.is_empty() && !selected.contains(&e.provider) {
            continue;
        }
        if !range.contains(e.time) {
            continue;
        }
        groups.entry((e.provider, e.model.clone())).or_default().push(e.clone());
    }
    let mut rows: Vec<ModelUsageRow> = groups
        .into_iter()
        .map(|((provider, model), entries)| ModelUsageRow { provider, model, entries })
        .collect();
    rows.sort_by(|a, b| b.sum().total.cmp(&a.sum().total).then(a.model.cmp(&b.model)));
    let mut sums = TokenSums::default();
    for row in &rows {
        let s = row.sum();
        sums.input += s.input;
        sums.cache_read += s.cache_read;
        sums.cache_write += s.cache_write;
        sums.cache_write_1h += s.cache_write_1h;
        sums.output += s.output;
        sums.total += s.total;
    }
    ModelUsage { rows, sums, incomplete: Vec::new() }
}

/// 紧凑数量：1234 → "1.2k"，1_200_000 → "1.2M"。
pub fn compact_tokens(n: u64) -> String {
    let f = n as f64;
    if f >= 1_000_000_000.0 {
        format!("{:.1}B", f / 1_000_000_000.0)
    } else if f >= 1_000_000.0 {
        format!("{:.1}M", f / 1_000_000.0)
    } else if f >= 1_000.0 {
        format!("{:.1}k", f / 1_000.0)
    } else {
        format!("{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(model: &str, time: i64, input: u64, output: u64) -> TokenEntry {
        TokenEntry { key: format!("{model}-{time}"), time, provider: Provider::Claude, model: model.into(), input, cache_read: 0, cache_write: 0, cache_write_1h: 0, output, reported_cost_usd: None }
    }

    #[test]
    fn totals_and_aggregation() {
        let e = TokenEntry { key: "a".into(), time: 10, provider: Provider::Claude, model: "m".into(), input: 1, cache_read: 2, cache_write: 3, cache_write_1h: 4, output: 5, reported_cost_usd: None };
        assert_eq!(e.total(), 15);
        let range = UsageDateRange { start_ms: 0, end_ms: 100 };
        let usage = aggregate(&[entry("opus", 10, 100, 50), entry("opus", 20, 100, 50), entry("sonnet", 30, 10, 10)], &[], range);
        assert_eq!(usage.rows.len(), 2);
        assert_eq!(usage.rows[0].model, "opus");
        assert_eq!(usage.rows[0].sum().total, 300);
        assert_eq!(usage.sums.total, 320);
    }

    #[test]
    fn date_range_filters() {
        let range = UsageDateRange { start_ms: 100, end_ms: 200 };
        assert!(range.contains(150));
        assert!(!range.contains(50));
        assert!(!range.contains(250));
        let usage = aggregate(&[entry("m", 50, 1, 1), entry("m", 150, 1, 1)], &[], range);
        assert_eq!(usage.sums.total, 2);
    }

    #[test]
    fn compact_formats() {
        assert_eq!(compact_tokens(999), "999");
        assert_eq!(compact_tokens(1234), "1.2k");
        assert_eq!(compact_tokens(1_500_000), "1.5M");
        assert_eq!(compact_tokens(2_000_000_000), "2.0B");
    }
}
