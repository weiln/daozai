//! 价格表与成本估算。
//!
//! - 内置一份常见模型的公开价（USD / 百万 token），按模型名前缀匹配。
//! - 成本 = 源自带 cost ? 直接用 : input*In + output*Out + cache_read*CacheRead + cache_write*CacheWrite。
//! - 查不到价 → 该记录计为“未定价”（不猜，显示 ≥）。

use super::model::TokenEntry;
use std::collections::HashMap;

/// 每百万 token 的价格（USD）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prices {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// 价格表：模型名（小写）→ 价格。支持前缀匹配（如 "claude-3-5-sonnet" 命中 "claude-3-5-sonnet-20241022"）。
#[derive(Clone, Debug, Default)]
pub struct Table {
    /// 精确/前缀名 → 价格，按插入顺序尝试最长的前缀。
    entries: Vec<(String, Prices)>,
    /// 用户/配置覆盖，优于内置。
    overrides: HashMap<String, Prices>,
}

impl Table {
    /// 内置公开价（USD / 1M tokens）。数值为约数，用户可在设置中覆盖。
    pub fn builtin() -> Self {
        let p = |i: f64, o: f64, cr: f64, cw: f64| Prices { input: i, output: o, cache_read: cr, cache_write: cw };
        let entries = vec![
            // Anthropic Claude
            ("claude-3-5-sonnet".to_string(), p(3.0, 15.0, 0.3, 3.75)),
            ("claude-3-5-haiku".to_string(), p(0.8, 4.0, 0.08, 1.0)),
            ("claude-3-opus".to_string(), p(15.0, 75.0, 1.5, 18.75)),
            ("claude-3-sonnet".to_string(), p(3.0, 15.0, 0.3, 3.75)),
            ("claude-3-haiku".to_string(), p(0.25, 1.25, 0.03, 0.3)),
            ("claude-sonnet-4".to_string(), p(3.0, 15.0, 0.3, 3.75)),
            ("claude-opus-4".to_string(), p(15.0, 75.0, 1.5, 18.75)),
            // OpenAI
            ("gpt-4o".to_string(), p(2.5, 10.0, 1.25, 2.5)),
            ("gpt-4o-mini".to_string(), p(0.15, 0.6, 0.075, 0.3)),
            ("gpt-4-turbo".to_string(), p(10.0, 30.0, 0.0, 0.0)),
            ("gpt-4.1".to_string(), p(2.0, 8.0, 0.5, 0.0)),
            ("o3".to_string(), p(2.0, 8.0, 0.5, 0.0)),
            ("o4-mini".to_string(), p(1.1, 4.4, 0.275, 0.0)),
            // DeepSeek
            ("deepseek-chat".to_string(), p(0.27, 1.1, 0.07, 0.27)),
            ("deepseek-reasoner".to_string(), p(0.55, 2.19, 0.14, 0.55)),
            // Moonshot Kimi
            ("kimi-k2".to_string(), p(0.6, 2.5, 0.15, 0.6)),
            ("moonshot-v1-8k".to_string(), p(0.17, 0.17, 0.0, 0.0)),
            // Google Gemini
            ("gemini-2.0-flash".to_string(), p(0.1, 0.4, 0.025, 0.0)),
            ("gemini-1.5-pro".to_string(), p(1.25, 5.0, 0.3125, 0.0)),
            ("gemini-1.5-flash".to_string(), p(0.075, 0.3, 0.01875, 0.0)),
        ];
        Self { entries, overrides: HashMap::new() }
    }

    /// 覆盖某模型的价格（用户配置）。
    pub fn set_override(&mut self, model: &str, prices: Prices) {
        self.overrides.insert(model.to_ascii_lowercase(), prices);
    }

    /// 查价：先覆盖，再按最长的内置前缀匹配。
    pub fn lookup(&self, model: &str) -> Option<Prices> {
        let m = model.to_ascii_lowercase();
        if let Some(p) = self.overrides.get(&m) {
            return Some(*p);
        }
        // 最长的前缀优先。
        let mut best: Option<(&String, Prices)> = None;
        for (name, price) in &self.entries {
            if m.starts_with(name.as_str()) || m.contains(name.as_str()) {
                if best.map_or(true, |(b, _)| name.len() > b.len()) {
                    best = Some((name, *price));
                }
            }
        }
        best.map(|(_, p)| p)
    }

    /// 估算单条记录的成本（USD）。返回 None 表示未定价。
    pub fn cost(&self, entry: &TokenEntry) -> Option<f64> {
        if let Some(c) = entry.reported_cost_usd {
            return Some(c.max(0.0));
        }
        let p = self.lookup(&entry.model)?;
        let per = 1_000_000.0;
        let cost = (entry.input as f64) * p.input / per
            + (entry.output as f64) * p.output / per
            + (entry.cache_read as f64) * p.cache_read / per
            + (entry.cache_write + entry.cache_write_1h) as f64 * p.cache_write / per;
        Some(cost.max(0.0))
    }
}

/// 成本汇总：金额 + 已报告/已估算/未定价条数。
#[derive(Clone, Copy, Debug, Default)]
pub struct CostSummary {
    pub amount: f64,
    pub reported: usize,
    pub estimated: usize,
    pub unpriced: usize,
}
impl CostSummary {
    pub fn add(&mut self, cost: Option<f64>, reported: bool) {
        match cost {
            Some(c) => {
                self.amount += c;
                if reported {
                    self.reported += 1;
                } else {
                    self.estimated += 1;
                }
            }
            None => self.unpriced += 1,
        }
    }
    /// 是否有至少一条记录参与计价。
    pub fn priced(&self) -> bool {
        self.reported + self.estimated > 0
    }
    /// 金额展示：USD → "$x.xx"；有未定价条目时前缀 "≥"。
    pub fn text(&self) -> String {
        if !self.priced() {
            return "—".into();
        }
        let mut s = if self.unpriced > 0 { "≥".to_string() } else { String::new() };
        s.push_str(&format!("${:.2}", self.amount));
        s
    }
}

/// 汇总一组记录的成本。
pub fn summarize(table: &Table, entries: &[TokenEntry]) -> CostSummary {
    let mut sum = CostSummary::default();
    for e in entries {
        sum.add(table.cost(e), e.reported_cost_usd.is_some());
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::Provider;

    fn e(model: &str, input: u64, output: u64, cost: Option<f64>) -> TokenEntry {
        TokenEntry { key: "k".into(), time: 0, provider: Provider::Claude, model: model.into(), input, cache_read: 0, cache_write: 0, cache_write_1h: 0, output, reported_cost_usd: cost }
    }

    #[test]
    fn prefix_lookup() {
        let t = Table::builtin();
        assert!(t.lookup("claude-3-5-sonnet-20241022").is_some());
        assert!(t.lookup("gpt-4o-mini").is_some());
        assert!(t.lookup("unknown-model-zzz").is_none());
        // gpt-4o 要优于 gpt-4o-mini 不被误配（这里 mini 名字更长，含 gpt-4o 前缀也含 mini；最长前缀应命中 mini）
        let mini = t.lookup("gpt-4o-mini").unwrap();
        assert!((mini.input - 0.15).abs() < 1e-9);
    }

    #[test]
    fn cost_math_and_reported() {
        let t = Table::builtin();
        // 1M input + 1M output of claude-3-5-sonnet = 3 + 15 = 18 USD
        let c = t.cost(&e("claude-3-5-sonnet", 1_000_000, 1_000_000, None)).unwrap();
        assert!((c - 18.0).abs() < 1e-6);
        // 源自带 cost 优先
        let c2 = t.cost(&e("whatever", 999, 999, Some(0.5))).unwrap();
        assert!((c2 - 0.5).abs() < 1e-9);
        // 未定价
        assert!(t.cost(&e("no-such-model", 1, 1, None)).is_none());
    }

    #[test]
    fn summary_text() {
        let t = Table::builtin();
        let entries = vec![
            e("claude-3-5-sonnet", 1_000_000, 0, None),
            e("unknown", 100, 100, None),
        ];
        let s = summarize(&t, &entries);
        assert_eq!(s.estimated, 1);
        assert_eq!(s.unpriced, 1);
        assert!(s.text().starts_with('≥'));
        assert!(s.text().contains("$3.00"));
    }

    #[test]
    fn override_beats_builtin() {
        let mut t = Table::builtin();
        t.set_override("claude-3-5-sonnet", Prices { input: 0.0, output: 0.0, cache_read: 0.0, cache_write: 0.0 });
        let c = t.cost(&e("claude-3-5-sonnet", 1_000_000, 1_000_000, None)).unwrap();
        assert_eq!(c, 0.0);
    }
}
