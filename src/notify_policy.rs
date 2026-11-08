//! 通知智能聚合的纯逻辑：免打扰时段判定、按应用静音、摘要文案。
//!
//! 与具体通知来源解耦，可单测。岛仔的提醒（微信/QQ/邮箱/内部提醒）统一经过这里
//! 决定是否弹出、是否静音、是否折叠进摘要。

/// 免打扰时段（时:分，[start, end)）。跨午夜时 start > end。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuietHours {
    pub start_min: u16,
    pub end_min: u16,
}
impl QuietHours {
    /// 从小时数构造（0..=23，end 可到 24）。
    pub fn from_hours(start: u8, end: u8) -> Self {
        Self { start_min: start as u16 * 60, end_min: (end as u16).min(24) * 60 }
    }
    /// 是否处于免打扰：now 为当日分钟数（0..1440）。
    pub fn contains(&self, now_min: u16) -> bool {
        if self.start_min == self.end_min {
            return false; // 相同视为关闭
        }
        if self.start_min < self.end_min {
            now_min >= self.start_min && now_min < self.end_min
        } else {
            // 跨午夜：[start, 1440) ∪ [0, end)
            now_min >= self.start_min || now_min < self.end_min
        }
    }
    pub fn label(&self) -> String {
        let fmt = |m: u16| format!("{:02}:{:02}", m / 60, m % 60);
        format!("{}-{}", fmt(self.start_min), fmt(self.end_min))
    }
}

/// 一条通知的来源应用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NotifyApp {
    Internal,
    Wechat,
    Qq,
    Mail,
    Feishu,
    DingTalk,
}
impl NotifyApp {
    pub const ALL: [Self; 6] = [Self::Internal, Self::Wechat, Self::Qq, Self::Mail, Self::Feishu, Self::DingTalk];
    pub fn key(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Wechat => "wechat",
            Self::Qq => "qq",
            Self::Mail => "mail",
            Self::Feishu => "feishu",
            Self::DingTalk => "dingtalk",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Internal => "岛仔提醒",
            Self::Wechat => "微信",
            Self::Qq => "QQ",
            Self::Mail => "邮箱",
            Self::Feishu => "飞书",
            Self::DingTalk => "钉钉",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == s)
    }
    /// 在 `ALL` 中的下标（用于摘要计数数组）。
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|a| *a == self).unwrap_or(0)
    }
}

/// 通知策略决策。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// 正常弹出（并计入摘要）。
    Show,
    /// 静音：不弹出，但计入摘要。
    Muted,
    /// 免打扰：不弹出，计入摘要。
    Quiet,
}
impl Decision {
    pub fn popup(self) -> bool {
        matches!(self, Decision::Show)
    }
}

/// 决定一条通知如何处理。
pub fn decide(quiet_on: bool, quiet: QuietHours, now_min: u16, muted: bool) -> Decision {
    if muted {
        return Decision::Muted;
    }
    if quiet_on && quiet.contains(now_min) {
        return Decision::Quiet;
    }
    Decision::Show
}

/// 摘要文案：如 "5 条微信未读"。
pub fn digest_text(app: NotifyApp, count: usize) -> String {
    if count == 0 {
        String::new()
    } else if app == NotifyApp::Mail {
        format!("{count} 封新邮件")
    } else {
        format!("{count} 条{}未读", app.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_hours_cross_midnight() {
        let q = QuietHours::from_hours(23, 7);
        assert!(q.contains(23 * 60 + 30)); // 23:30
        assert!(q.contains(2 * 60)); // 02:00
        assert!(!q.contains(12 * 60)); // 12:00
        let day = QuietHours::from_hours(12, 14);
        assert!(day.contains(13 * 60));
        assert!(!day.contains(11 * 60));
        // 相同 = 关闭
        assert!(!QuietHours::from_hours(9, 9).contains(9 * 60));
    }

    #[test]
    fn decide_priority() {
        let q = QuietHours::from_hours(22, 8);
        assert_eq!(decide(true, q, 23 * 60, false), Decision::Quiet);
        assert_eq!(decide(true, q, 12 * 60, false), Decision::Show);
        assert_eq!(decide(true, q, 12 * 60, true), Decision::Muted);
        assert!(!decide(true, q, 23 * 60, false).popup());
        assert!(decide(false, q, 23 * 60, false).popup());
    }

    #[test]
    fn digest_forms() {
        assert_eq!(digest_text(NotifyApp::Wechat, 5), "5 条微信未读");
        assert_eq!(digest_text(NotifyApp::Mail, 3), "3 封新邮件");
        assert_eq!(digest_text(NotifyApp::Wechat, 0), "");
    }
}
