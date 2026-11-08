//! AI Agent 生命周期状态灯。
//!
//! 参考 fly-island 的契约：不做进程/CPU 探测（Claude Code 可能跑在 VSCode 终端、
//! 常驻空闲），而是通过各工具的生命周期钩子写“运行标记文件”与 `agent-status.json`，
//! 再由岛仔轮询读取：
//! - 运行中（running）→ 红，动态
//! - 完成 / 空闲 / 缺省 / 非法 → 绿
//! - 失败（failed）或运行标记超过 10 分钟未更新（派生 stale）→ 黄
//!
//! 钩子脚本安装在 `%APPDATA%\Daozai\agent-hooks\`，并写入 Codex / Claude Code 的配置。
//! 纯逻辑（解析、派生、配置块生成）与平台无关，可单测。

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// 运行标记超过此时长仍未更新，视为“卡住”（stale → 黄）。
pub const STALE_AFTER_MS: i64 = 10 * 60 * 1000;

/// 受支持的 AI 编程工具。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Codex,
    ClaudeCode,
    Qoder,
    WorkBuddy,
    Trae,
}

impl AgentKind {
    pub const ALL: [Self; 5] = [Self::Codex, Self::ClaudeCode, Self::Qoder, Self::WorkBuddy, Self::Trae];
    pub fn key(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claudeCode",
            Self::Qoder => "qoder",
            Self::WorkBuddy => "workbuddy",
            Self::Trae => "trae",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
            Self::Qoder => "Qoder",
            Self::WorkBuddy => "WorkBuddy",
            Self::Trae => "Trae",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key().eq_ignore_ascii_case(s))
    }
    /// 该工具有可验证的钩子机制；没有的仅能显示“无法监控”。
    pub fn hookable(self) -> bool {
        matches!(self, Self::Codex | Self::ClaudeCode)
    }
}

/// 任务阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Running,
    Completed,
    Failed,
    Stale,
}
impl Phase {
    pub fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "stale" => Self::Stale,
            _ => Self::Idle,
        }
    }
    /// 收起到最“严重”的状态用于聚合成一枚圆点：红 > 橙 > 绿。
    pub fn severity(self) -> u8 {
        match self {
            Self::Failed | Self::Stale => 2,
            Self::Running => 1,
            _ => 0,
        }
    }
}

/// 单个工具的状态。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TaskStatus {
    pub phase: Phase,
    pub updated_at: i64,
}
impl Default for Phase {
    fn default() -> Self {
        Phase::Idle
    }
}

/// 一次快照：各工具状态 + 版本号（变化即 +1，供 UI 判断是否重绘）。
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub agents: HashMap<AgentKind, TaskStatus>,
    pub version: u64,
}
impl Snapshot {
    /// 聚合到最严重的一枚圆点状态；无任何工具时返回 None。
    pub fn aggregate(&self) -> Option<Phase> {
        let mut worst: Option<Phase> = None;
        for status in self.agents.values() {
            let phase = status.phase;
            if worst.map_or(true, |w| phase.severity() > w.severity()) {
                worst = Some(phase);
            }
        }
        worst
    }
    pub fn phase_of(&self, kind: AgentKind) -> Phase {
        self.agents.get(&kind).map(|s| s.phase).unwrap_or(Phase::Idle)
    }
}

// ---------------------------------------------------------------- 纯逻辑：解析与派生
/// 从 `agent-status.json` 文本解析各工具状态。非法/缺省返回空。
pub fn parse_status_json(text: &str) -> HashMap<AgentKind, TaskStatus> {
    let mut out = HashMap::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return out };
    let Some(obj) = v.as_object() else { return out };
    for kind in AgentKind::ALL {
        if let Some(entry) = obj.get(kind.key()).and_then(|e| e.as_object()) {
            let phase = entry.get("phase").and_then(|p| p.as_str()).map(Phase::parse).unwrap_or(Phase::Idle);
            let updated_at = entry.get("updatedAt").and_then(|u| u.as_i64()).unwrap_or(0);
            out.insert(kind, TaskStatus { phase, updated_at });
        }
    }
    out
}

/// 由标记文件与状态文件派生最终阶段。
/// - running 标记比状态更新 → running；若标记存在超过 STALE_AFTER_MS → stale。
/// - 否则以状态文件 phase 为准。
pub fn derive_phase(status: &TaskStatus, running_marker_ms: Option<i64>, now_ms: i64) -> Phase {
    if let Some(marker) = running_marker_ms {
        // 状态文件里有比标记更新的结论（例如刚完成的 completed）→ 以状态为准。
        if status.phase != Phase::Running && status.updated_at > 0 && status.updated_at >= marker {
            return status.phase;
        }
        if now_ms.saturating_sub(marker) >= STALE_AFTER_MS {
            return Phase::Stale;
        }
        return Phase::Running;
    }
    status.phase
}

// ---------------------------------------------------------------- 路径
/// 钩子脚本目录。
pub fn hooks_dir() -> PathBuf {
    crate::model::config_dir().join("agent-hooks")
}
/// 状态文件。
pub fn status_path() -> PathBuf {
    hooks_dir().join("agent-status.json")
}
fn running_flag(kind: AgentKind) -> PathBuf {
    hooks_dir().join(format!("agent-{}-running.flag", kind.key()))
}
fn hold_flag(kind: AgentKind) -> PathBuf {
    hooks_dir().join(format!("agent-{}-hold.flag", kind.key()))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn file_mtime_ms(path: &Path) -> Option<i64> {
    std::fs::metadata(path).ok()?.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64)
}

/// 读取当前时间下的一份快照（纯读取，不加锁）。
pub fn read_snapshot() -> Snapshot {
    let now = now_ms();
    let mut agents = std::fs::read_to_string(status_path())
        .ok()
        .map(|t| parse_status_json(&t))
        .unwrap_or_default();
    for kind in AgentKind::ALL {
        let status = agents.get(&kind).cloned().unwrap_or_default();
        let marker = file_mtime_ms(&running_flag(kind));
        let phase = derive_phase(&status, marker, now);
        agents.insert(kind, TaskStatus { phase, updated_at: status.updated_at.max(marker.unwrap_or(0)) });
    }
    Snapshot { agents, version: 0 }
}

// ---------------------------------------------------------------- 采集服务
/// 每 200ms 轮询标记/状态文件，变化时通过通道推给 UI。
pub struct Service {
    rx: mpsc::Receiver<Snapshot>,
    _stop: mpsc::Sender<()>,
}
impl Service {
    pub fn start(repaint: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let (stop, stopped) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("daozai-agent-status".into())
            .spawn(move || {
                let mut last_sig: Option<HashMap<AgentKind, Phase>> = None;
                let mut version = 0u64;
                loop {
                    if stopped.try_recv().is_ok() {
                        break;
                    }
                    let mut snap = read_snapshot();
                    let sig: HashMap<_, _> = snap.agents.iter().map(|(k, v)| (*k, v.phase)).collect();
                    if last_sig.as_ref() != Some(&sig) {
                        version += 1;
                        snap.version = version;
                        last_sig = Some(sig);
                        let _ = tx.send(snap);
                        repaint();
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            })
            .ok();
        Self { rx, _stop: stop }
    }
    /// 尝试取最新快照；无变化返回 None。
    pub fn try_recv(&self) -> Option<Snapshot> {
        self.rx.try_recv().ok()
    }
}

// ---------------------------------------------------------------- 钩子安装器
/// 内置脚本：写/删运行标记。参数为工具 key。
pub const RUNNING_SCRIPT: &str = r#"@echo off
rem 岛仔 AI 状态灯：记录“运行中”标记。用法：daozai-agent-running.cmd <codex|claudeCode>
setlocal
set DIR=%~dp0
if "%~1"=="" exit /b 0
copy /y nul "%DIR%agent-%~1-running.flag" >nul 2>nul
del "%DIR%agent-%~1-hold.flag" >nul 2>nul
exit /b 0
"#;

/// 内置脚本：写状态（completed/failed），删运行标记，保留短时 hold。
pub const STATUS_SCRIPT: &str = r#"param([Parameter(Mandatory=$true)][string]$Provider,[Parameter(Mandatory=$true)][string]$Phase)
$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
$running = Join-Path $dir ("agent-{0}-running.flag" -f $Provider)
$hold = Join-Path $dir ("agent-{0}-hold.flag" -f $Provider)
$status = Join-Path $dir "agent-status.json"
$mutex = New-Object System.Threading.Mutex($false, "Daozai.AgentStatus")
$null = $mutex.WaitOne(2000)
try {
  $now = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
  $obj = @{}
  if (Test-Path $status) { try { $existing = Get-Content $status -Raw | ConvertFrom-Json } catch { $existing = $null } } else { $existing = $null }
  if ($existing) { $existing.PSObject.Properties | ForEach-Object { $obj[$_.Name] = $_.Value } }
  $obj[$Provider] = [pscustomobject]@{ phase = $Phase; updatedAt = $now }
  $obj['updatedAt'] = $now
  $json = $obj | ConvertTo-Json -Depth 6
  $tmp = "$status.tmp"
  Set-Content -Path $tmp -Value $json -Encoding UTF8
  Move-Item -Force $tmp $status
  if (Test-Path $running) { Remove-Item -Force $running }
  if (Test-Path $hold) { Remove-Item -Force $hold }
} finally { $null = $mutex.ReleaseMutex(); $mutex.Dispose() }
if ($env:DAOZAI_HOOK_RESPONSE -eq "1") { Write-Output '{"continue":true,"suppressOutput":true}' }
exit 0
"#;

/// Codex 配置块（`~/.codex/config.toml`）。
pub fn codex_hook_block(running_cmd: &str, status_ps1: &str) -> String {
    format!(
        "# BEGIN Daozai Agent Status Hooks\n\
[[hooks.UserPromptSubmit]]\n\
[[hooks.UserPromptSubmit.hooks]]\n\
type = \"command\"\n\
command = {run}\n\
command_windows = {run}\n\
statusMessage = \"更新岛仔 Agent 状态\"\n\
\n\
[[hooks.Stop]]\n\
[[hooks.Stop.hooks]]\n\
type = \"command\"\n\
command = {stop}\n\
command_windows = {stop}\n\
statusMessage = \"更新岛仔 Agent 状态\"\n\
# END Daozai Agent Status Hooks\n",
        run = toml_string(running_cmd),
        stop = toml_string(status_ps1),
    )
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Claude Code 的 hooks JSON 条目（合并进 `~/.claude/settings.json` 的 `hooks`）。
pub fn claude_hook_json(running_cmd: &str, status_ps1: &str) -> serde_json::Value {
    let run = serde_json::json!({"type":"command","command":"cmd.exe","args":["/C", running_cmd]});
    let run_pre = run.clone();
    let stop = serde_json::json!({"type":"command","command":"powershell.exe","args":["-NoProfile","-ExecutionPolicy","Bypass","-File", status_ps1, "claudeCode", "completed"]});
    let fail = serde_json::json!({"type":"command","command":"powershell.exe","args":["-NoProfile","-ExecutionPolicy","Bypass","-File", status_ps1, "claudeCode", "failed"]});
    serde_json::json!({
        "UserPromptSubmit": [{"hooks":[run]}],
        "PreToolUse": [{"matcher":"*","hooks":[run_pre]}],
        "Stop": [{"hooks":[stop]}],
        "StopFailure": [{"hooks":[fail]}],
    })
}

/// 把 `block` 插入/替换进 `content` 的哨兵块（BEGIN/END 之间）。已有则替换。
pub fn splice_sentinel_block(content: &str, block: &str) -> String {
    const BEGIN: &str = "# BEGIN Daozai Agent Status Hooks";
    const END: &str = "# END Daozai Agent Status Hooks";
    if let Some(start) = content.find(BEGIN) {
        if let Some(rel_end) = content[start..].find(END) {
            let end = start + rel_end + END.len();
            // 连同其后换行一起替换。
            let after = content[end..].trim_start_matches(['\r','\n']);
            let mut out = String::new();
            out.push_str(&content[..start]);
            out.push_str(block);
            out.push('\n');
            out.push_str(after);
            return out;
        }
    }
    let mut out = content.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(block);
    out
}

/// 合并 Claude hooks：保留用户已有的其它 hook 事件与条目，替换 Daozai 的（含旧签名的）。
pub fn merge_claude_hooks(settings: &mut serde_json::Value, hooks: serde_json::Value) {
    let root = match settings.as_object_mut() {
        Some(o) => o,
        None => {
            *settings = serde_json::json!({});
            settings.as_object_mut().unwrap()
        }
    };
    let entry = root.entry("hooks").or_insert_with(|| serde_json::json!({}));
    if !entry.is_object() {
        *entry = serde_json::json!({});
    }
    let obj = entry.as_object_mut().unwrap();
    if let Some(new) = hooks.as_object() {
        for (event, value) in new {
            // 与用户已有条目合并；Daozai 条目以 command 是否含 daozai-agent 判别。
            let existing = obj.get(event).and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let kept: Vec<serde_json::Value> = existing
                .into_iter()
                .filter(|e| !hook_entry_is_daozai(e))
                .collect();
            let mut merged = kept;
            if let Some(arr) = value.as_array() {
                merged.extend(arr.iter().cloned());
            }
            obj.insert(event.clone(), serde_json::Value::Array(merged));
        }
    }
}

fn hook_entry_is_daozai(entry: &serde_json::Value) -> bool {
    let text = entry.to_string().to_ascii_lowercase();
    text.contains("daozai-agent") || text.contains("focusd-agent")
}

/// 安装结果（供设置 UI 展示）。
#[derive(Clone, Debug, Default)]
pub struct InstallResult {
    pub scripts_dir: String,
    pub status_path: String,
    pub codex_config: String,
    pub claude_config: String,
    pub notes: Vec<String>,
}

/// 安装/修复钩子脚本与配置。Windows 下写盘并改配置；其它平台返回说明。
#[cfg(windows)]
pub fn install() -> Result<InstallResult, String> {
    let dir = hooks_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建脚本目录：{e}"))?;
    write_crlf(&dir.join("daozai-agent-running.cmd"), RUNNING_SCRIPT)?;
    write_crlf(&dir.join("daozai-agent-status.ps1"), STATUS_SCRIPT)?;

    let running_cmd = dir.join("daozai-agent-running.cmd");
    let status_ps1 = dir.join("daozai-agent-status.ps1");
    let mut notes = Vec::new();

    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
    let codex_cfg = home.join(".codex").join("config.toml");
    let claude_cfg = home.join(".claude").join("settings.json");

    // Codex：cmd.exe /C "<running cmd>" codex
    let codex_run = format!("cmd.exe /C \"{}\" codex", running_cmd.display());
    let codex_stop = format!("powershell.exe -NoProfile -ExecutionPolicy Bypass -File \"{}\" codex completed", status_ps1.display());
    match install_codex(&codex_cfg, &codex_hook_block(&codex_run, &codex_stop)) {
        Ok(()) => {}
        Err(e) => notes.push(format!("Codex：{e}")),
    }
    // Claude Code：exec 形式（cmd.exe + 参数），避免各终端 shell 差异。
    let claude_run = format!("\"{}\" claudeCode", running_cmd.display());
    match install_claude(&claude_cfg, &claude_hook_json(&claude_run, &status_ps1.display().to_string())) {
        Ok(()) => {}
        Err(e) => notes.push(format!("Claude Code：{e}")),
    }
    // UI 里对 Qoder / WorkBuddy / Trae 直接说明：无公开钩子，暂无法监控。

    Ok(InstallResult {
        scripts_dir: dir.display().to_string(),
        status_path: status_path().display().to_string(),
        codex_config: codex_cfg.display().to_string(),
        claude_config: claude_cfg.display().to_string(),
        notes,
    })
}

#[cfg(not(windows))]
pub fn install() -> Result<InstallResult, String> {
    Err("AI 状态灯钩子目前仅支持 Windows".into())
}

fn write_crlf(path: &Path, content: &str) -> Result<(), String> {
    let normalized = content.replace("\r\n", "\n").replace('\n', "\r\n");
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, normalized).map_err(|e| format!("写入 {} 失败：{e}", tmp.display()))?;
    std::fs::rename(&tmp, path).or_else(|_| {
        std::fs::remove_file(path).ok();
        std::fs::rename(&tmp, path)
    }).map_err(|e| format!("替换 {} 失败：{e}", path.display()))
}

#[cfg(windows)]
fn install_codex(path: &Path, block: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let current = std::fs::read_to_string(path).unwrap_or_default();
    let next = splice_sentinel_block(&current, block);
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, next).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn install_claude(path: &Path, hooks: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut settings: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    merge_claude_hooks(&mut settings, hooks.clone());
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// 模拟某工具的运行中→完成（供设置页“自检”与测试用）。
pub fn simulate(kind: AgentKind, phase: Phase) {
    let _ = std::fs::create_dir_all(hooks_dir());
    let now = now_ms();
    let mut agents = std::fs::read_to_string(status_path()).ok().map(|t| parse_status_json(&t)).unwrap_or_default();
    agents.insert(kind, TaskStatus { phase, updated_at: now });
    let obj: serde_json::Map<String, serde_json::Value> = agents
        .iter()
        .map(|(k, v)| {
            (
                k.key().to_string(),
                serde_json::json!({"phase": match v.phase { Phase::Running => "running", Phase::Completed => "completed", Phase::Failed => "failed", Phase::Stale => "stale", Phase::Idle => "idle" }, "updatedAt": v.updated_at}),
            )
        })
        .collect();
    let mut root = serde_json::Map::new();
    for (k, v) in obj {
        root.insert(k, v);
    }
    root.insert("updatedAt".into(), serde_json::json!(now));
    let json = serde_json::Value::Object(root);
    if let Ok(text) = serde_json::to_string_pretty(&json) {
        let tmp = status_path().with_extension("tmp");
        let mut f = match std::fs::File::create(&tmp) { Ok(f) => f, Err(_) => return };
        let _ = f.write_all(text.as_bytes());
        let _ = std::fs::rename(&tmp, status_path());
    }
    match phase {
        Phase::Running => {
            let _ = std::fs::write(running_flag(kind), b"");
            let _ = std::fs::remove_file(hold_flag(kind));
        }
        _ => {
            let _ = std::fs::remove_file(running_flag(kind));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_phase_and_json() {
        assert_eq!(Phase::parse("running"), Phase::Running);
        assert_eq!(Phase::parse("garbage"), Phase::Idle);
        let json = r#"{"codex":{"phase":"running","updatedAt":1000},"claudeCode":{"phase":"completed","updatedAt":2000}}"#;
        let map = parse_status_json(json);
        assert_eq!(map.get(&AgentKind::Codex).unwrap().phase, Phase::Running);
        assert_eq!(map.get(&AgentKind::ClaudeCode).unwrap().phase, Phase::Completed);
        assert!(!map.contains_key(&AgentKind::Qoder));
        assert!(parse_status_json("bad").is_empty());
    }

    #[test]
    fn derive_running_completed_stale() {
        let idle = TaskStatus::default();
        // 有新鲜运行标记 → running
        assert_eq!(derive_phase(&idle, Some(1000), 2000), Phase::Running);
        // 运行标记超过 10 分钟 → stale
        assert_eq!(derive_phase(&idle, Some(0), STALE_AFTER_MS + 1), Phase::Stale);
        // 无标记 → 以状态文件为准
        assert_eq!(derive_phase(&TaskStatus { phase: Phase::Failed, updated_at: 10 }, None, 99), Phase::Failed);
        // 状态比标记更新（刚完成）→ completed
        assert_eq!(derive_phase(&TaskStatus { phase: Phase::Completed, updated_at: 5000 }, Some(4000), 6000), Phase::Completed);
    }

    #[test]
    fn aggregate_picks_most_severe() {
        let mut snap = Snapshot::default();
        snap.agents.insert(AgentKind::Codex, TaskStatus { phase: Phase::Idle, updated_at: 0 });
        snap.agents.insert(AgentKind::ClaudeCode, TaskStatus { phase: Phase::Running, updated_at: 0 });
        assert_eq!(snap.aggregate(), Some(Phase::Running));
        snap.agents.insert(AgentKind::Qoder, TaskStatus { phase: Phase::Failed, updated_at: 0 });
        assert_eq!(snap.aggregate(), Some(Phase::Failed));
    }

    #[test]
    fn splice_replaces_existing_block() {
        // 用带哨兵的块测试幂等替换。
        let b1 = codex_hook_block("cmd1", "ps1");
        let b2 = codex_hook_block("cmd2", "ps2");
        let first = splice_sentinel_block("existing-user-line\n", &b1);
        assert!(first.contains("cmd1"));
        assert!(first.contains("existing-user-line"));
        let second = splice_sentinel_block(&first, &b2);
        assert!(second.contains("cmd2"));
        assert!(!second.contains("cmd1"));
        // 只保留一个哨兵块。
        assert_eq!(second.matches("# BEGIN Daozai Agent Status Hooks").count(), 1);
    }

    #[test]
    fn merge_claude_preserves_user_hooks_and_replaces_daozai() {
        let mut settings = serde_json::json!({
            "hooks": {
                "Stop": [
                    {"hooks":[{"type":"command","command":"user-script.sh"}]},
                    {"hooks":[{"type":"command","command":"old-daozai-agent-status.ps1"}]}
                ]
            }
        });
        let hooks = claude_hook_json("daozai-agent-running.cmd claudeCode", "daozai-agent-status.ps1");
        merge_claude_hooks(&mut settings, hooks);
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        // 用户条目保留，旧 Daozai 条目被替换掉。
        assert!(stop.iter().any(|e| e.to_string().contains("user-script.sh")));
        assert!(stop.iter().any(|e| e.to_string().contains("claudeCode")));
        // UserPromptSubmit 也写入了。
        assert!(settings["hooks"]["UserPromptSubmit"].as_array().is_some());
    }
}
