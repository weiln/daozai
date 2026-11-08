//! 本地会话日志增量解析引擎（Claude Code / Codex 等 JSONL）。
//!
//! 关键机制（贴合 BrimDeck 的 LogReader）：
//! - 每文件记住已解析的字节偏移与大小；只解析新增行。文件变短/改写则从头重解析。
//! - 整行先做关键字预筛（`"usage"` / `"token_count"` / `"turn_context"`），不含则跳过，
//!   避免逐行 JSON 解析巨大的工具输出。
//! - 跨文件重复的消息按 key 取最完整（total 最大）者。
//! - 超过 `MAX_LINE` 的单行跳过并计入错误，不无限膨胀缓冲。
//!
//! 提供纯解析函数 `parse_claude_line` / `parse_codex_line` 供单测，不依赖文件系统。

use super::model::{TokenEntry, Provider as UsageProvider};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// 单行最大字节；超过则视为不可读（巨大 tool result）。
pub const MAX_LINE: usize = 32 << 20;

fn keyword_hit(line: &str) -> bool {
    line.contains("\"usage\"") || line.contains("\"token_count\"") || line.contains("\"turn_context\"")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// 解析 Claude Code 的一行 JSONL。返回本行的 TokenEntry（若含 usage）与最新 model。
/// Claude 每行是一个 content block，携带整条消息的 usage；同消息重复出现，靠 key 去重。
pub fn parse_claude_line(line: &str, model_hint: &mut String) -> Option<TokenEntry> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    // 记录 model（message.model 或顶层 model）。
    if let Some(m) = v.pointer("/message/model").and_then(|x| x.as_str()) {
        *model_hint = m.to_string();
    }
    let usage = v.pointer("/message/usage").or_else(|| v.get("usage"))?;
    let num = |k: &str| usage.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let input = num("input_tokens");
    let output = num("output_tokens");
    let cache_read = num("cache_read_input_tokens");
    let cache_write = num("cache_creation_input_tokens");
    if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
        return None;
    }
    let time = v.get("timestamp").and_then(|t| t.as_str()).and_then(parse_iso_ms)
        .or_else(|| v.get("timestamp").and_then(|t| t.as_i64()))
        .unwrap_or_else(now_ms);
    // 去重键：message id + request id（存在时），否则时间+model+总量。
    let msg_id = v.pointer("/message/id").and_then(|x| x.as_str())
        .or_else(|| v.get("requestId").and_then(|x| x.as_str()))
        .or_else(|| v.get("uuid").and_then(|x| x.as_str()));
    let model = if model_hint.is_empty() { "unknown".to_string() } else { model_hint.clone() };
    let key = match msg_id {
        Some(id) => format!("claude:{id}"),
        None => format!("claude:{time}:{model}:{}", input + output + cache_read + cache_write),
    };
    Some(TokenEntry {
        key, time, provider: UsageProvider::Claude, model,
        input, cache_read, cache_write, cache_write_1h: 0, output,
        reported_cost_usd: usage.get("cost").and_then(|c| c.as_f64()),
    })
}

/// 解析 Codex 的一行 JSONL（`type=="event_msg" && payload.type=="token_count"`）。
/// Codex 给出累积 `total_token_usage`，用差分得每轮。`prev` 保存上一累积值。
pub fn parse_codex_line(line: &str, prev: &mut (u64, u64, u64, u64), model_hint: &mut String) -> Option<TokenEntry> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if ty == "turn_context" {
        if let Some(m) = v.pointer("/payload/model").and_then(|x| x.as_str()) {
            *model_hint = m.to_string();
        }
        return None;
    }
    if ty != "event_msg" {
        return None;
    }
    if v.pointer("/payload/type").and_then(|t| t.as_str()) != Some("token_count") {
        return None;
    }
    let info = v.pointer("/payload/info")?;
    // 优先 last_token_usage（增量），否则用 total 差分。
    let (input, cached, output) = if let Some(last) = info.get("last_token_usage") {
        (
            last.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
            last.get("cached_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
            last.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
        )
    } else if let Some(total) = info.get("total_token_usage") {
        let ti = total.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
        let tc = total.get("cached_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
        let to = total.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
        let (pi, pc, po, _) = *prev;
        *prev = (ti, tc, to, 0);
        (ti.saturating_sub(pi), tc.saturating_sub(pc), to.saturating_sub(po))
    } else {
        return None;
    };
    if input == 0 && output == 0 && cached == 0 {
        return None;
    }
    let time = v.get("timestamp").and_then(|t| t.as_str()).and_then(parse_iso_ms)
        .or_else(|| v.get("timestamp").and_then(|t| t.as_i64()))
        .unwrap_or_else(now_ms);
    let model = if model_hint.is_empty() { "unknown".to_string() } else { model_hint.clone() };
    Some(TokenEntry {
        key: format!("codex:{time}:{input}:{output}"),
        time, provider: UsageProvider::Codex, model,
        input, cache_read: cached, cache_write: 0, cache_write_1h: 0, output,
        reported_cost_usd: None,
    })
}

/// 解析 RFC3339 时间串到毫秒。
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp_millis())
}

// ---------------------------------------------------------------- 文件扫描器
#[derive(Default, Clone)]
struct FileState {
    offset: u64,
    len: u64,
    /// 该文件已产出的 key（用于文件内去重）。
    keys: HashSet<String>,
    model: String,
    /// Codex 累积差分状态。
    codex_prev: (u64, u64, u64, u64),
    errors: u64,
}

/// 一次扫描的全部结果。
#[derive(Default)]
pub struct ScanResult {
    pub entries: Vec<TokenEntry>,
    pub notes: Vec<(UsageProvider, String)>,
}

/// 增量扫描器：维护每文件状态与全局去重。
pub struct Scanner {
    files: HashMap<PathBuf, FileState>,
    /// 全局 key → entry（跨运行累积，取最完整）。
    merged: HashMap<String, TokenEntry>,
    last_roots: (Option<PathBuf>, Option<PathBuf>),
}

impl Scanner {
    pub fn new() -> Self {
        Self { files: HashMap::new(), merged: HashMap::new(), last_roots: (None, None) }
    }

    /// 重扫所有数据源，返回当前累计的所有条目。
    pub fn scan(&mut self) -> ScanResult {
        let mut notes = Vec::new();
        let (claude_root, codex_root) = roots();
        // 数据源变化（换机/换账号）时清空。
        if self.last_roots != (claude_root.clone(), codex_root.clone()) {
            self.files.clear();
            self.merged.clear();
            self.last_roots = (claude_root.clone(), codex_root.clone());
        }
        if let Some(root) = &claude_root {
            self.scan_root(root, UsageProvider::Claude, &mut notes);
        }
        if let Some(root) = &codex_root {
            self.scan_root(root, UsageProvider::Codex, &mut notes);
        }
        let mut entries: Vec<TokenEntry> = self.merged.values().cloned().collect();
        entries.sort_by_key(|e| e.time);
        ScanResult { entries, notes }
    }

    fn scan_root(&mut self, root: &Path, provider: UsageProvider, notes: &mut Vec<(UsageProvider, String)>) {
        if !root.is_dir() {
            notes.push((provider, "未找到本地会话记录".into()));
            return;
        }
        let mut files = 0usize;
        let mut errors = 0u64;
        for path in walk_jsonl(root) {
            files += 1;
            errors += self.parse_file(&path, provider);
        }
        notes.push((provider, format!("已扫描 {files} 个记录文件{}", if errors > 0 { format!("，{errors} 行无法读取") } else { String::new() })));
    }

    /// 增量解析一个文件，返回累计错误行数。
    fn parse_file(&mut self, path: &Path, provider: UsageProvider) -> u64 {
        let Ok(meta) = std::fs::metadata(path) else { return 0 };
        let len = meta.len();
        let st = self.files.entry(path.to_path_buf()).or_default();
        // 文件变短或改写 → 从头重来。
        if len < st.len {
            *st = FileState { model: st.model.clone(), ..Default::default() };
        }
        if len == st.len && st.offset > 0 {
            return st.errors;
        }
        let Ok(data) = std::fs::read(path) else { return st.errors };
        let start = st.offset.min(data.len() as u64) as usize;
        let mut pos = start;
        while pos < data.len() {
            let Some(nl) = data[pos..].iter().position(|b| *b == b'\n') else { break };
            let line_end = pos + nl;
            if line_end - pos > MAX_LINE {
                st.errors += 1;
                pos = line_end + 1;
                continue;
            }
            let line = String::from_utf8_lossy(&data[pos..line_end]);
            let line = line.trim_start_matches('\u{feff}');
            if keyword_hit(line) {
                let parsed = match provider {
                    UsageProvider::Claude => parse_claude_line(line, &mut st.model),
                    UsageProvider::Codex => parse_codex_line(line, &mut st.codex_prev, &mut st.model),
                    _ => None,
                };
                if let Some(entry) = parsed {
                    if !st.keys.contains(&entry.key) {
                        st.keys.insert(entry.key.clone());
                        Self::merge(&mut self.merged, entry);
                    } else {
                        // 已有同 key：若本行更完整则替换。
                        Self::merge(&mut self.merged, entry);
                    }
                }
            }
            pos = line_end + 1;
        }
        st.offset = pos as u64;
        st.len = len;
        st.errors
    }

    /// 跨文件合并：同 key 取 total 最大者。
    fn merge(merged: &mut HashMap<String, TokenEntry>, entry: TokenEntry) {
        match merged.get(&entry.key) {
            Some(old) if old.total() >= entry.total() => {}
            _ => {
                merged.insert(entry.key.clone(), entry);
            }
        }
    }
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

/// Claude Code 与 Codex 的数据根目录（遵循与额度发现一致的环境变量覆盖）。
fn roots() -> (Option<PathBuf>, Option<PathBuf>) {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"))
        .join("projects");
    let codex = std::env::var_os("CODEX_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
        .join("sessions");
    (claude.is_dir().then_some(claude), codex.is_dir().then_some(codex))
}

/// 递归列出目录下所有 .jsonl 文件（限制深度，跳过符号链接）。
pub fn walk_jsonl(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0u32)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 8 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push((path, depth + 1));
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_line_parses_usage() {
        let mut model = String::new();
        let line = r#"{"type":"assistant","timestamp":"2024-10-01T10:00:00Z","message":{"id":"msg_1","model":"claude-3-5-sonnet-20241022","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":10,"cache_creation_input_tokens":5}}}"#;
        let e = parse_claude_line(line, &mut model).unwrap();
        assert_eq!(e.input, 100);
        assert_eq!(e.output, 50);
        assert_eq!(e.cache_read, 10);
        assert_eq!(e.cache_write, 5);
        assert_eq!(e.total(), 165);
        assert_eq!(e.model, "claude-3-5-sonnet-20241022");
        // 无 usage 的行返回 None
        assert!(parse_claude_line(r#"{"type":"user"}"#, &mut model).is_none());
    }

    #[test]
    fn codex_line_delta() {
        let mut prev = (0, 0, 0, 0);
        let mut model = String::new();
        parse_codex_line(r#"{"type":"turn_context","payload":{"model":"gpt-4o"}}"#, &mut prev, &mut model);
        assert_eq!(model, "gpt-4o");
        let l1 = r#"{"type":"event_msg","timestamp":"2024-10-01T10:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":40}}}}"#;
        let e1 = parse_codex_line(l1, &mut prev, &mut model).unwrap();
        assert_eq!((e1.input, e1.cache_read, e1.output), (100, 10, 40));
        let l2 = r#"{"type":"event_msg","timestamp":"2024-10-01T10:01:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":250,"cached_input_tokens":30,"output_tokens":90}}}}"#;
        let e2 = parse_codex_line(l2, &mut prev, &mut model).unwrap();
        assert_eq!((e2.input, e2.cache_read, e2.output), (150, 20, 50));
    }

    #[test]
    fn keyword_prefilter() {
        assert!(keyword_hit(r#"{"usage":{}}"#));
        assert!(keyword_hit(r#"{"payload":{"type":"token_count"}}"#));
        assert!(keyword_hit(r#"{"type":"turn_context"}"#));
        assert!(!keyword_hit(r#"{"type":"message","content":"hello"}"#));
    }

    #[test]
    fn iso_parse() {
        assert!(parse_iso_ms("2024-10-01T10:00:00Z").is_some());
        assert!(parse_iso_ms("not-a-date").is_none());
    }
}
