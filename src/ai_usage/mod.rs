//! AI 编程工具用量与成本分析（移植自 BrimDeck 的用量子系统）。
//!
//! - 读取各工具本地会话记录（JSONL / SQLite），解析出逐个调用的 token 用量。
//! - 按 (工具, 模型) 与日期范围聚合，结合价格表估算成本。
//! - 只读本地记录或公开价格，不改变工具登录状态；隐私优先，仅本地处理。
//!
//! 模块划分：
//! - `model`：`TokenEntry`（互斥 token 类别）、`UsageMetric`、`ModelUsage` 聚合。
//! - `logs`：增量解析引擎（Claude Code / Codex 等 JSONL）。
//! - `pricing`：价格表（内置兜底 + 可选联网目录）与成本估算。

pub mod logs;
pub mod model;
pub mod pricing;

pub use model::{TokenEntry, UsageDateRange, Provider as UsageProvider};

use std::sync::mpsc;
use std::time::Duration;

/// 一次用量快照：各工具解析出的条目 + 不完整/错误说明 + 版本号。
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub entries: Vec<TokenEntry>,
    /// 各工具的说明（已扫描文件数、错误行数等）。
    pub notes: Vec<(UsageProvider, String)>,
    pub version: u64,
}

/// 后台采集服务：启动时全量、之后增量解析本地日志。
pub struct Service {
    rx: mpsc::Receiver<Snapshot>,
    _stop: mpsc::Sender<()>,
}
impl Service {
    pub fn start(repaint: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let (stop, stopped) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("daozai-ai-usage".into())
            .spawn(move || {
                let mut scanner = logs::Scanner::new();
                let mut version = 0u64;
                loop {
                    if stopped.try_recv().is_ok() {
                        break;
                    }
                    let scan = scanner.scan();
                    version += 1;
                    let mut snap = Snapshot { entries: scan.entries, notes: scan.notes, version };
                    snap.version = version;
                    let _ = tx.send(snap);
                    repaint();
                    // 30 秒增量刷新。
                    for _ in 0..300 {
                        if stopped.try_recv().is_ok() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            })
            .ok();
        Self { rx, _stop: stop }
    }
    pub fn try_recv(&self) -> Option<Snapshot> {
        self.rx.try_recv().ok()
    }
}
