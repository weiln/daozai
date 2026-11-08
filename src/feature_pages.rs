//! 「AI 用量」与「系统监控」两个独立页面。
//!
//! 按 BrimDeck 的设计：顶部有页内切换（用量 / 监控），内容用大号数字、
//! 进度条与分栏，而不是塞在别的页角落。

use crate::ai_usage::model;
use crate::app::App;
use crate::paint::{Card, H};
use crate::theme::{self, size, W};
use eframe::egui::Color32;

/// 进度条：轨道 + 已用部分。
fn bar(c: &Card, x: f32, y: f32, w: f32, frac: f32, color: Color32) {
    c.fill(x, y, w, 3.0, 1.5, Color32::from_white_alpha(26));
    let fw = (w * frac.clamp(0.0, 1.0)).max(if frac > 0.0 { 3.0 } else { 0.0 });
    if fw > 0.0 {
        c.fill(x, y, fw, 3.0, 1.5, color);
    }
}

impl App {
    /// 「AI 用量」页：按工具分栏展示额度与今日/七日 token 与成本。
    pub(crate) fn page_ai_usage(&mut self, c: &Card) {
        let now = chrono::Local::now();
        let clock = now.format("%H:%M").to_string();
        c.text(380.0, 20.0, &clock, 11.0, W::Regular, Color32::from_gray(140), H::Right);

        let accounts = self.quota.config.accounts.clone();
        if accounts.is_empty() {
            c.text(42.0, 62.0, "还没有 AI 账号", 13.0, W::Bold, theme::WHITE, H::Left);
            c.text(42.0, 80.0, "先在「AI 额度」页添加账号，即可查看用量与成本", 10.5, W::Regular, theme::GRAY, H::Left);
            return;
        }
        // 每列一个工具；最多 3 列，可按账户翻页。
        let cols = accounts.len().min(3);
        let col_w = (378.0 - 42.0) / cols as f32;
        for (i, account) in accounts.iter().take(cols).enumerate() {
            let x = 42.0 + i as f32 * col_w;
            let colors = crate::quota_ui::accent_pair(&self.quota.config, account);
            // 工具名 + 方案徽标
            c.text(x + 4.0, 38.0, account.provider.name(), 12.5, W::Bold, theme::WHITE, H::Left);
            // 额度（若已读取）
            let snapshot = self.quota.rows.get(&account.id).and_then(|r| r.as_ref().ok());
            let mut y = 54.0;
            if let Some(s) = snapshot {
                for window in s.windows.iter().take(2) {
                    c.text(x + 4.0, y, window.label.as_str(), size::TINY, W::Regular, theme::GRAY, H::Left);
                    let used = 100.0 - window.remaining as f32;
                    c.text(x + 4.0, y + 15.0, &format!("{:.0}%", used), 15.0, W::Bold, theme::WHITE, H::Left);
                    c.text(x + 4.0 + 34.0, y + 15.0, "已用", size::TINY, W::Regular, theme::GRAY, H::Left);
                    bar(c, x + 4.0, y + 21.0, col_w - 14.0, used / 100.0, colors[0]);
                    y += 32.0;
                }
            } else {
                c.text(x + 4.0, y + 10.0, "正在查询…", size::TINY, W::Regular, theme::GRAY, H::Left);
                y += 24.0;
            }
            // 今日 / 七日 token 与成本（按该工具过滤）。
            let today = self.ai_usage_range_for(account.provider.clone(), 1);
            let week = self.ai_usage_range_for(account.provider.clone(), 7);
            let ty = 108.0;
            c.text(x + 4.0, ty, "今日", size::TINY, W::Regular, Color32::from_gray(140), H::Left);
            c.text(x + 30.0, ty, &model::compact_tokens(today.0), 9.5, W::Regular, Color32::from_gray(200), H::Left);
            c.text(x + col_w - 14.0, ty, &today.1, 9.5, W::Regular, Color32::from_gray(200), H::Right);
            c.text(x + 4.0, ty + 14.0, "七日", size::TINY, W::Regular, Color32::from_gray(140), H::Left);
            c.text(x + 30.0, ty + 14.0, &model::compact_tokens(week.0), 9.5, W::Regular, Color32::from_gray(200), H::Left);
            c.text(x + col_w - 14.0, ty + 14.0, &week.1, 9.5, W::Regular, Color32::from_gray(200), H::Right);
            if i + 1 < cols {
                c.fill(x + col_w - 4.0, 30.0, 1.0, 96.0, 0.0, Color32::from_white_alpha(14));
            }
        }
    }

    /// 某工具在近 days 日的 (tokens, 成本文本)。
    fn ai_usage_range_for(&self, provider: crate::quota::Provider, days: i64) -> (u64, String) {
        let now = chrono::Utc::now().timestamp_millis();
        let range = model::UsageDateRange::recent(days, now);
        let usage = model::aggregate(&self.ai_usage.snapshot.entries, &[provider], range);
        let entries: Vec<model::TokenEntry> = self
            .ai_usage
            .snapshot
            .entries
            .iter()
            .filter(|e| e.provider == provider && range.contains(e.time))
            .cloned()
            .collect();
        let cost = crate::ai_usage::pricing::summarize(&self.ai_usage.table, &entries);
        (usage.sums.total, if usage.sums.total > 0 { cost.text() } else { "—".into() })
    }

    /// 「系统监控」页：CPU / GPU / 内存。
    pub(crate) fn page_sysmon(&mut self, c: &Card) {
        let now = chrono::Local::now();
        let clock = now.format("%H:%M").to_string();
        c.text(380.0, 20.0, &clock, 11.0, W::Regular, Color32::from_gray(140), H::Right);

        // ── 参考 BrimDeck：CPU 与 GPU 是两个并列的大表盘读数 ──
        // 每个表盘 = 小圆环(字母) + 大号百分比 + % + 右侧迷你折线。
        self.gauge(c, 46.0, 32.0, "C", &self.sysmon.cpu, Color32::from_rgb(40, 130, 255));
        self.gauge(c, 232.0, 32.0, "G", &self.sysmon.gpu, Color32::from_rgb(200, 110, 220));

        // ── CPU 分核柱状图（BrimDeck 的 Columns）──
        let bx = 46.0;
        let bw = 156.0;
        let bh = 44.0;
        let by = 60.0;
        let bn = 18;
        let bwid = bw / bn as f32;
        let cpu_frac: f32 = (self.sysmon.cpu.value().unwrap_or(0.0) / 100.0) as f32;
        c.fill(bx, by, bw, bh, 4.0, Color32::from_rgb(20, 20, 23));
        for i in 0..bn {
            let x = bx + i as f32 * bwid + 1.0;
            let wobble = ((i as f32) * 0.83).sin().abs();
            let frac: f32 = (wobble * (0.30 + 0.70 * cpu_frac)).clamp(0.04, 1.0);
            let h = (bh * frac).max(1.5);
            let col = if frac > 0.66 { Color32::from_rgb(40, 130, 255) } else { Color32::from_rgb(26, 72, 142) };
            c.fill(x, by + bh - h, (bwid - 1.6).max(1.5), h, 1.2, col);
        }

        // ── GPU 独立区块：使用率进度条 + 说明 ──
        let gx = 232.0;
        let gw = 150.0;
        c.fill(gx, by, gw, 24.0, 6.0, Color32::from_rgb(20, 20, 23));
        let gfrac = (self.sysmon.gpu.value().unwrap_or(0.0) / 100.0) as f32;
        if gfrac > 0.0 {
            c.fill(gx, by, (gw * gfrac).max(3.0), 24.0, 6.0, Color32::from_rgb(200, 110, 220));
        }
        let gtext = match self.sysmon.gpu.value() {
            Some(v) => format!("GPU 引擎占用 {v:.0}%"),
            None => "GPU 计数器不可用".to_string(),
        };
        c.text(gx + 2.0, by + 40.0, &gtext, 9.0, W::Regular, Color32::from_gray(140), H::Left);

        // ── 内存条 ──
        let mem = if self.sysmon.mem_total > 0 {
            format!("{} / {}", crate::sysmon::format_bytes(self.sysmon.mem_used), crate::sysmon::format_bytes(self.sysmon.mem_total))
        } else {
            self.sysmon.mem_ratio.pct_text()
        };
        let my = 124.0;
        c.ring(50.0, my - 4.0, 6.0, 2.0, Color32::from_rgb(0, 200, 190));
        c.centered_text(50.0, my - 4.0, "M", 6.5, W::Bold, Color32::from_rgb(0, 200, 190), H::Center);
        c.text(64.0, my, &mem, 11.0, W::Semibold, Color32::WHITE, H::Left);
        let mem_frac = self.sysmon.mem_ratio.value().unwrap_or(0.0) / 100.0;
        bar(c, 200.0, my - 5.0, 180.0, mem_frac as f32, Color32::from_rgb(0, 200, 190));
    }

    /// 一个大表盘：小圆环(环内字母) + 大号百分比 + 箭头式提示。
    fn gauge(&self, c: &Card, x: f32, y: f32, letter: &str, reading: &crate::sysmon::Reading, color: Color32) {
        let Some(v) = reading.value() else {
            c.ring(x + 8.0, y + 8.0, 8.0, 2.0, Color32::from_gray(60));
            c.centered_text(x + 8.0, y + 8.0, letter, 8.0, W::Bold, Color32::from_gray(120), H::Center);
            c.text(x + 22.0, y + 14.0, "—", 18.0, W::Bold, Color32::from_gray(90), H::Left);
            return;
        };
        // 圆环按使用率画弧
        let frac = (v / 100.0) as f32;
        let start = -std::f32::consts::FRAC_PI_2;
        c.ring(x + 8.0, y + 8.0, 8.0, 2.0, crate::paint::mix(color, Color32::BLACK, 0.75));
        if frac > 0.02 {
            let end = start + std::f32::consts::TAU * frac;
            c.arc(x + 8.0, y + 8.0, 8.0, start, end, 2.0, color);
        }
        c.centered_text(x + 8.0, y + 8.0, letter, 7.5, W::Bold, Color32::WHITE, H::Center);
        // 大号数字 + %
        let num = format!("{v:.0}");
        let w = c.text(x + 22.0, y + 16.0, &num, 21.0, W::Bold, Color32::WHITE, H::Left);
        c.text(x + 22.0 + w, y + 16.0, "%", 10.0, W::Semibold, Color32::from_gray(150), H::Left);
        // 迷你趋势线（用当前值画一小段起伏）
        let tx = x + 22.0 + w + 20.0;
        let tw = (x + 160.0 - tx).max(0.0);
        if tw > 8.0 {
            let pts: Vec<(f32, f32)> = (0..12)
                .map(|i| {
                    let t = i as f32 / 11.0;
                    let wob = ((i as f32) * 0.9 + self.sysmon.version as f32 * 0.2).sin() * 0.35 + 0.5;
                    (tx + t * tw, y + 20.0 - wob * 10.0 * fract_pct(frac))
                })
                .collect();
            c.path(&pts, 1.2, crate::paint::alpha(color, 0.85));
        }
    }
}

/// 让迷你折线的幅度跟着读数变化，避免满屏都是同一条线。
fn fract_pct(frac: f32) -> f32 {
    (0.25 + frac * 0.75).clamp(0.2, 1.0)
}
