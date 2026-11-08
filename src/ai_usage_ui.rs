//! AI 用量与成本分析：设置页详细面板 + 收起态 HUD 摘要。

use crate::ai_usage::{self, model, pricing};
use crate::app::App;
use crate::quota::Provider;
use eframe::egui::{self, Color32};

/// 用量面板状态。
pub struct State {
    pub snapshot: ai_usage::Snapshot,
    pub table: pricing::Table,
    /// 统计口径天数：1 / 7 / 30。
    pub days: i64,
    /// 选中的工具（空表示全部）。
    pub selected: Vec<Provider>,
}
impl Default for State {
    fn default() -> Self {
        Self { snapshot: Default::default(), table: pricing::Table::builtin(), days: 7, selected: Vec::new() }
    }
}
impl State {
    pub fn load() -> Self {
        let s = crate::model::Settings::load();
        Self { days: s.ai_usage_days, ..Default::default() }
    }

    /// 当前口径下的聚合结果。
    pub fn usage(&self) -> model::ModelUsage {
        let now = chrono::Utc::now().timestamp_millis();
        let range = model::UsageDateRange::recent(self.days.max(1), now);
        model::aggregate(&self.snapshot.entries, &self.selected, range)
    }

    /// 当前口径的成本汇总。
    pub fn cost(&self) -> pricing::CostSummary {
        let now = chrono::Utc::now().timestamp_millis();
        let range = model::UsageDateRange::recent(self.days.max(1), now);
        let entries: Vec<model::TokenEntry> = self
            .snapshot
            .entries
            .iter()
            .filter(|e| (self.selected.is_empty() || self.selected.contains(&e.provider)) && range.contains(e.time))
            .cloned()
            .collect();
        pricing::summarize(&self.table, &entries)
    }

    /// 收起态 HUD 文本：如 "近7日 123k tok · $0.12"；无数据返回 None。
    pub fn hud_text(&self) -> Option<String> {
        let sums = self.usage().sums;
        if sums.total == 0 {
            return None;
        }
        let cost = self.cost();
        Some(format!("近{}日 {} tok · {}", self.days, model::compact_tokens(sums.total), cost.text()))
    }
}

impl App {
    /// 供外壳推送用量快照。
    pub fn set_ai_usage(&mut self, snapshot: ai_usage::Snapshot) {
        self.ai_usage.snapshot = snapshot;
    }

    /// 设置页里的用量与成本详细面板。
    pub(crate) fn ai_usage_settings(&mut self, ui: &mut egui::Ui) {
        use crate::extras::{settings_card, settings_hint, settings_label};
        settings_label(ui, "AI 用量与成本");
        // 口径与总开关
        settings_card(ui, |ui| {
            let mut enabled = self.settings.ai_usage_enabled;
            if ui.checkbox(&mut enabled, "统计本地会话的 token 用量与成本").changed() {
                self.settings.ai_usage_enabled = enabled;
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("统计口径").size(12.0).color(Color32::from_gray(120)));
                for (d, label) in [(1i64, "今日"), (7, "近 7 日"), (30, "近 30 日")] {
                    if ui.selectable_label(self.ai_usage.days == d, label).clicked() {
                        self.ai_usage.days = d;
                        self.settings.ai_usage_days = d;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("工具").size(12.0).color(Color32::from_gray(120)));
                let all_on = self.ai_usage.selected.is_empty();
                if ui.selectable_label(all_on, "全部").clicked() {
                    self.ai_usage.selected.clear();
                }
                for p in [Provider::Claude, Provider::Codex, Provider::Kimi, Provider::DeepSeek] {
                    let on = self.ai_usage.selected.contains(&p);
                    if ui.selectable_label(on, p.name()).clicked() {
                        if on {
                            self.ai_usage.selected.retain(|x| *x != p);
                        } else {
                            self.ai_usage.selected.push(p);
                        }
                    }
                }
            });
            let mut hud = self.settings.ai_usage_hud;
            if ui.checkbox(&mut hud, "收起时在岛上显示用量摘要").changed() {
                self.settings.ai_usage_hud = hud;
            }
        });

        // 汇总
        let sums = self.ai_usage.usage().sums;
        let cost = self.ai_usage.cost();
        settings_card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("总 Tokens").size(11.0).color(Color32::from_gray(130)));
                ui.label(egui::RichText::new(model::compact_tokens(sums.total)).size(20.0).strong());
                ui.add_space(12.0);
                ui.label(egui::RichText::new("预估成本").size(11.0).color(Color32::from_gray(130)));
                ui.label(egui::RichText::new(cost.text()).size(20.0).strong());
            });
            ui.label(
                egui::RichText::new(format!(
                    "输入 {} · 输出 {} · 缓存读 {} · 缓存写 {}",
                    model::compact_tokens(sums.input),
                    model::compact_tokens(sums.output),
                    model::compact_tokens(sums.cache_read),
                    model::compact_tokens(sums.cache_write + sums.cache_write_1h),
                ))
                .size(11.0)
                .color(Color32::from_gray(130)),
            );
        });

        // 按模型明细
        let usage = self.ai_usage.usage();
        if usage.rows.is_empty() {
            settings_hint(ui, "暂无本地会话记录（或所选工具/口径无数据）。仅统计本机 Claude Code / Codex 等会话。");
        } else {
            settings_label(ui, "按模型");
            settings_card(ui, |ui| {
                for row in usage.rows.iter().take(12) {
                    let s = row.sum();
                    let c = pricing::summarize(&self.ai_usage.table, &row.entries);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{} · {}", row.provider.name(), row.model)).size(12.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(c.text()).size(12.0).color(Color32::from_gray(150)));
                            ui.label(egui::RichText::new(model::compact_tokens(s.total)).size(12.0).strong());
                            let _ = s.input;
                        });
                    });
                }
            });
            if !self.ai_usage.snapshot.notes.is_empty() {
                let notes: Vec<String> = self.ai_usage.snapshot.notes.iter().map(|(p, n)| format!("{}：{}", p.name(), n)).collect();
                settings_hint(ui, &notes.join("；"));
            }
        }
    }
}
