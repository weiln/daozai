# 岛仔 · 功能扩展开发计划

> 依据《功能扩展需求清单》制定。参考实现：
> - fly-island — https://github.com/flyl1u/fly-island（Tauri + Rust + TS）
> - BrimDeck — https://github.com/MoFeng2223/BrimDeck（C# / .NET WPF）
>
> 两份参考仓库已克隆到 `E:\github\_refs\fly-island` 与 `E:\github\_refs\BrimDeck`（浅克隆，仅用于阅读源码）。
> 本文中所有对参考实现的结论均来自实际阅读源码，非仓库描述。
> 另有一份 fly-island 逐文件（file:line）分析报告落于 `E:\github\_refs\fly-island\_report\fly-island-feature-report.md`；一份 BrimDeck「用量与成本」逐文件分析报告落在 `docs\BrimDeck-Usage-Cost-Report.md`（含 Rust 映射清单），可作实现期对照。

> **状态（v0.7.0 之后）：** 本文的 P0-1「AI Agent 生命周期状态灯」与 P0-2「AI 编程工具用量与成本分析」已随功能下线撤回 —— `src/agent_status.rs`、`src/ai_usage/`、`src/ai_usage_ui.rs` 与「AI 用量」页均已删除。以下章节保留作历史设计记录，不要再据此新增代码；如需重新引入，请先确认产品方向。

---

## 0. 现状与基线

### 0.1 岛仔（本项目）的既有架构

| 关注点 | 现有实现 | 文件 |
|---|---|---|
| UI 框架 | Rust + egui（eframe 0.31，glow/wgpu） | `Cargo.toml` |
| 面板/页面 | 8 个固定页：音乐 / 专注 / 饮水 / 置物架 / 待办 / 邮箱 / AI 额度 / 日程；`render_page()` 按 `self.page` 分派 | `src/app.rs:2411-2419`，`src/pages.rs`，`src/quota_ui.rs` |
| 平台无关层 | `app`/`pages` 与窗口外壳解耦，录制器与正式程序共用 | `src/lib.rs`，`src/app.rs` |
| 后台服务模式 | `Svc::start()` 起线程 + `mpsc` 通道，主线程 `try_recv` 轮询，`repaint` 触发重绘（额度、邮件、日历、蓝牙、SMTC 都是这个模式） | `src/quota.rs:254-307`（范例） |
| 设置 | `Settings` 结构 + `settings.txt` 的 KV 序列化（`load`/`save` 手写逐字段） | `src/model.rs:185-400` |
| 设置窗口 | 独立 viewport，`SETTINGS_GROUPS` 8 组，`settings_tab` 索引切换 | `src/extras.rs:1035-1038, 1041-1188` |
| 剪贴板 | 仅文字；`AddClipboardFormatListener` 事件驱动；密码管理器标记自动排除；`clipboard.json` | `src/clipboard.rs` |
| 系统采样 | `system::observe()` 后台线程 → `Observation` 通道（电源/网络/输入法/音量/蓝牙） | `src/system.rs:26-70` |
| 凭据存储 | `keyring`（`windows-native`）已在依赖中（邮件 IMAP 用） | `Cargo.toml:101` |
| AI 额度 | 8 个 Provider（Codex/Claude/Kimi/GrokBot/Gemini/ZCode/WorkBuddy/DeepSeek）本地凭据发现 + 联网查余额 | `src/quota.rs`，`src/quota_ui.rs` |
| 收起态 | `BAR_W=163×30` 药丸，或极简 `44×4.5`；岛体边缘渲染 HUD | `src/app.rs:27-41` |

### 0.2 页面/可见性约束（重要）

- `visible_pages: [bool; 8]`、`page_order: [usize; 8]`、`PAGE_NAMES: [&str; 8]`、多处 `[1,2,3,5,4,7,11,8]` 硬编码映射 —— **新增第 9 个“页面”会牵动大量下标假设**。
- **结论**：除“系统监控面板”需要成为可切换的第 9 页（需谨慎扩数组）外，其余新功能尽量以**子页 / 独立窗口 / 收起态 HUD 元素 / 置物架子标签**的形式挂接，避免动 8 页模型。

### 0.3 参考实现的关键结论（源码级）

**A. fly-island — AI Agent 状态灯（`src-tauri/src/agent_status/`，`docs/agent-status-hooks.md`）**
- 不做进程/CPU 探测（Claude Code 可能跑在 VSCode 终端、常驻空闲）。
- 生命周期钩子写入 `%USERPROFILE%\.codex\config.toml`（TOML `[[hooks.UserPromptSubmit]]` / `[[hooks.Stop]]` 块，用 `# BEGIN/END FocuSD Agent Status Hooks` 包裹可重复安装）与 `%USERPROFILE%\.claude\settings.json`（JSON `hooks.UserPromptSubmit / PreToolUse / Stop / StopFailure`）。
- 运行时状态灯靠**标记文件**：`agent-codex-running.flag`、`agent-claudeCode-running.flag`（快速、无 JSON 开销），轮询 200ms。turn 结束时脚本删标记、写 `agent-status.json`：
  ```json
  {"codex":{"phase":"running|completed|failed|idle|stale","taskId":"","updatedAt":1783584000000}, ...}
  ```
- 状态映射：`running`→红（动态）；`completed/idle/缺失/非法`→绿；`failed`/running 标记超 10 分钟无更新（派生 `stale`）→黄。
  - 精确配色（fly-island tokens，可直接沿用作参考）：idle 绿 `#35e985`、running 红 `#ff4d5e`、attention(failed/stale) 黄 `#ffd166`；圆点 ~9px + 14px 光晕。
- **hold 标记**（值得照搬）：turn 太快（<800ms）时，status 脚本写完 completed 后再留一个 hold 标记（含 `visibleUntil` 毫秒时间戳），让极短任务也能红闪约 800ms 再转绿——避免状态灯“跳个色就没了”。
- status 脚本用命名互斥锁（如 `Daozai.AgentStatus`）串行化对 `agent-status.json` 的读改写，避免并发 turn 冲突；写入用临时文件 + rename 原子替换。
- 前端轮询 200ms 且对相同快照去重（岛仔侧等价：采样线程 200ms，`try_recv` 后若快照未变则不 `request_repaint`）。
- **扩展空间**：清单要求覆盖 Codex、Claude Code、Qoder、WorkBuddy、Trae。fly-island 只做了 Codex/Claude 两家，其余四家需我们按同一契约自行补 hook 安装器（各自配置路径需在实现期确认）。
- 翻译快捷键同样用 `RegisterHotKey`（`MOD_NOREPEAT`）+ 消息窗口，keyring service 名 `com.focusd.island.translation`/`deepseek`，POST `https://api.deepseek.com/chat/completions`，`model: deepseek-flash`（见 `translation.rs:143-251`）。

**B. fly-island 的“用量与成本分析”——不存在；但 BrimDeck 有完整实现（P0 #2 以 BrimDeck 为参考）**
- fly-island：全仓搜索 `token/usage/pricing/cost`，Rust 侧仅 `qq_mail/content.rs` 命中（无关）——它没有此功能。
- **BrimDeck 有成熟、完整的“用量与成本分析”子系统**（`BrimDeck.Core\UsageService.cs`、`Usage.cs`、`UsageMetric.cs`、`Pricing.cs`、`ModelUsage.cs`、`LogReader.cs`、`UsagePanel.cs` 等）。P0 #2 应以 BrimDeck 为蓝本移植。要点见 §2.2。

**C. BrimDeck — 系统监控（`src/BrimDeck.Core/SystemMonitor.cs` 等）**
- 核心思想：`Reading<T>` —— 读数要么有值，要么 `Unavailable(reason)`，**永不伪造 0**；UI 显示 `—` + tooltip 说明原因；温度非法值（0/负/>125℃）静默丢弃。
- 单一采样线程（BelowNormal，STA），按需启动（打开面板才采），关闭后 **10s 逗留**再停；`Latest=null` 避免展示陈旧值；首采 300ms 后再采一次让速率类计数立即可读。
- 指标 API：CPU% 用 PDH `\Processor Information(*)\% Processor Time`（回退 `NtQuerySystemInformation` class 8）；GPU% 用 PDH `\GPU Engine(*)\Utilization Percentage`（每适配器取最忙引擎、跨进程求和，任务管理器口径）；内存 `GlobalMemoryStatusEx` + `K32GetPerformanceInfo`；磁盘 `GetDiskFreeSpaceEx` + `DeviceIoControl(IOCTL_STORAGE_QUERY_PROPERTY)`；温度 ACPI `\Thermal Zone Information(*)`、GPU 温度 `D3DKMTQueryAdapterInfo(KMTQAITYPE_ADAPTERPERFDATA)`；功耗 `\Energy Meter(*)\Power`。
- **普通用户无需管理员**即可读上述全部；**逐核 CPU 温度 & MSR 功耗需 PawnIO 内核驱动 + SYSTEM 服务 + 一次 UAC**（BrimDeck 的 `BrimDeck.Sensors` 服务）。
- 显示：¾ 圆环表盘（缺口在左下/右下以区别于额度环）+ 居中单字母（C/G/M），数字缓动，环形缓冲 60 样本画 sparkline，收起态极简数字。

---

## 1. 总体设计原则（承接需求 §3.1）

1. **模块化**：每个功能一个模块文件 / 子模块目录，UI 侧用函数据点挂接，不改动既有分派结构之外的部分。
2. **异步采集**：所有 I/O、轮询、联网统一走 `mpsc` 服务线程 + UI 线程 `try_recv`（沿用 `quota::Service` 范式）。禁止在 `update()` 内阻塞。
3. **配置驱动**：开关/参数/快捷键全部进 `Settings`（`model.rs` 的 `load`/`save` 各加一行），并进设置窗口对应分组。
4. **隐私优先**：翻译内容、剪贴板、用量数据本地处理或按需传输；凭据一律 `keyring`（Windows 凭据管理器）。沿用现有 `DPAPI`/`keyring` 实践。
5. **可降级**：任何平台能力缺失（传感器不可读、无 GPU、hook 目标工具未安装）都**静默跳过 / 显示 `—`**，绝不阻塞或崩溃主循环（BrimDeck 的 `Reading<T>` 原则）。
6. **平台隔离**：Windows 专属代码 `#[cfg(windows)]`，保持录制器/测试可跨平台编译。

---

## 2. P0 — 开发者工作流闭环

### P0-1 AI Agent 生命周期状态灯

**目标**：岛仔收起态以小圆点/图标颜色显示 Codex / Claude Code / Qoder / WorkBuddy / Trae 的运行中 / 已完成 / 需要关注；展开态详见面板。与现有 AI 额度页形成“用量 + 状态”双层视图。

**接入点**：`src/quota.rs` 旁新增 `src/agent_status.rs`（或 `quota/agent_status/` 子模块），`App` 增加 `agent_status: AgentStatus` 字段。

**数据结构**
```rust
enum Phase { Idle, Running, Completed, Failed, Stale }
struct AgentTask { phase: Phase, task_id: Option<String>, updated_at: i64 }
struct Snapshot { agents: HashMap<AgentKind, AgentTask>, updated_at: i64 } // AgentKind: Codex/ClaudeCode/Qoder/WorkBuddy/Trae
```

**Hook 安装器**（对齐 fly-island 契约，扩展四家）
- 脚本目录：`%APPDATA%\Daozai\agent-hooks\`（复用 `model::config_dir()` 风格另开子目录；脚本 `daozai-agent-running.cmd` + `daozai-agent-status.ps1`，`include_str!` 内置、写盘时统一 CRLF）。
- Codex：改 `~/.codex/config.toml`，插入 `# BEGIN/END Daozai Agent Status Hooks` 包裹块（`UserPromptSubmit` 写 running 标记；`Stop` 写 completed）。幂等、可重复安装、清理旧块。
- Claude Code：改 `~/.claude/settings.json`，`hooks.UserPromptSubmit / PreToolUse / Stop / StopFailure`（exec 形式：`cmd.exe` 或 `powershell.exe` + `args`，避免 shell 差异，见 fly-island doc §Notes）。
- Qoder / WorkBuddy / Trae：**实现期需先调研各自 hook 机制是否存在**；若某工具无 hook 能力，则该灯退化为“无法监控”并在设置里说明，不强行进程探测（遵循 fly-island 结论）。此为明确风险项（见 §7）。
- 安装入口：设置 → 新增「AI 状态灯」页，按钮「安装/修复 Hook」，展示各目标路径与结果。

**运行时**
- 采样线程每 200ms 读标记文件 + `agent-status.json`（缺省/非法 JSON → 全绿），派生 `stale`（running 标记 >10min）。经 `mpsc` 推 UI。
- **收起态**：岛体边缘小圆点（每 agent 一枚或聚合为一枚取最“严重”态：红>橙>绿），带呼吸动画表示 running。复用 `paint`/`shape` 的圆与 `anim::Spring`。
- **展开态面板**：挂到 AI 额度页作为子标签「状态」/或独立面板，列出每个 agent 的 phase、最近更新、清除按钮（对齐 `clear_agent_status`）。

**Settings 新增**：`agent_status_enabled: bool`、`agent_status_agents: [bool;5]`（按工具开关）。

**验收**：手动 smoke——跑 `daozai-agent-running.cmd codex` → 灯红；`daozai-agent-status.ps1 codex completed` → 绿；`... failed` → 黄；超过 10min 停留 running → 黄。

---

### P0-2 AI 编程工具用量与成本分析（参考 BrimDeck 完整移植）

**目标**：在“额度/余额”基础上扩展 tokens 消耗量与成本，按工具/模型/日期筛选；收起态显示当日（或当前会话）累计 tokens 或成本，展开态详细面板。只读本地记录或公开价格，**不改变工具登录状态**。

> **参考实现：BrimDeck 的用量子系统（已实读源码）—— 直接按此移植，不再从零设计。**

#### 2.2.1 BrimDeck 用量子系统的核心结构（移植蓝本）

**数据模型（`BrimDeck.Core\Usage.cs`）**
```rust
// 各 token 类别互斥；reasoning 已含在 output 内。
struct TokenEntry { key:String, time:i64, model:String,
    input:u64, cache_read:u64, cache_write:u64, cache_write_1h:u64, output:u64,
    reported_cost_usd:Option<f64> }         // 源里若自带 cost 就用它，不再用价格表估算
// total = input+cache_read+cache_write+cache_write_1h+output
```
**统一指标模型（`UsageMetric.cs`）** —— 额度与成本用同一枚举展示：
```rust
enum MetricKind { Percent, Balance, Spend, Count, Text }
struct UsageMetric { kind, label, percent, amount, total, used, currency, unit, unlimited, reset_at, at_least, total_is_target }
```
- 余额→用量、花费（Spend，可带 `≥` 下界）、计数、百分比共用一套渲染；金额格式化 USD→`$`、CNY→`¥`。这样岛仔现有“AI 额度”页可直接复用同一套卡片。

**价格表（`Pricing.cs`）—— 不是内置静态表，而是联网目录 + 本地覆盖**
- 主源：`https://openrouter.ai/api/v1/models`（USD **per token**，含 input/output/cache_read/cache_write，支持**阶梯价 `PriceTier`（按 prompt token 数分档）**）。缓存到 `model-prices.json`，日级刷新（`RefreshInterval=1 天`）。
- **手动价**：`manual-model-prices.json` 覆盖目录（用户在设置里加）。
- **官方价**：`official` 目录（各厂自己端点报的价，如 Kimi）。
- **`PriceRecovery`**：当源记录里自带 `ReportedCostUsd` 时，用最小二乘反推模型单价（`RecoveryStatus::{Recovered,TooFewRecords,Undetermined,Inconsistent}`）——没有公开价的长尾模型也能出成本。
- 找不到价格 → 计入 `Unpriced`，金额标“≥”（`CostSummary{ Amount, Reported, Estimated, Unpriced }`），**绝不假装 0**（与 `Reading<T>` 同精神）。
- **成本公式 `Estimate()`**：`cost = Reported ?? (Input×In + Output×Out + CacheRead×CacheRead + CacheWrite×CacheWrite [+ CacheWriteHour×CacheWriteHour])`；远端缺 cache 价 → 该记录计 unpriced（不猜）；手动价默认带 cache 价。
- 岛仔落地：`src/ai_usage/pricing.rs` 移植上述三源 + 反推；默认走 OpenRouter 目录，允许离线用手动/官方价。国内用户可加一份内置的 DeepSeek/Kimi/智谱等官方价兜底。内部统一用 **USD per token**（`rust_decimal`），仅 UI 显示时换算到每百万/人民币。

#### 2.2.2 本地用量数据源（`LogReader.cs` + `DataLocations.cs`，实测路径）

| 工具 | 路径 | 记录格式 |
|---|---|---|
| Claude Code | `~/.claude/projects/**/*.jsonl` | 每行一个 content block，含整条消息的 `usage{input_tokens,output_tokens,cache_creation_input_tokens,cache_read_input_tokens}`；同消息跨行/跨文件重复，按 key 去重取最完整 |
| Codex | `~/.codex/sessions/**`、`~/.codex/archived_sessions/**` | `type=event_msg / payload.type=token_count`，`payload.info.total_token_usage`（累积）与 `last_token_usage`（增量）——用**累积差分**得每轮；`turn_context` 行给 model；含 `rate_limits` 快照 |
| Kimi Code | `~/.kimi-code`（`KIMI_CODE_HOME`） | session 记录 |
| ZCode | `~/.zcode`（`ZCODE_HOME`） | |
| DSH（the agent harness） | `~/.dsh`（`DSH_HOME`） | |
| OpenCode | `$XDG_DATA_HOME\opencode` 或 `~/.local/share/opencode`（SQLite `opencode.db`） | 读数据库 |
| Cursor | `%APPDATA%\Cursor\User\globalStorage\state.vscdb`（SQLite） | token 明细 |
| Pi / Antigravity | `~/.pi/agent/sessions` / Antigravity 会话库 | 各自 JSONL |

> **重要匹配**：岛仔 `quota::Provider` 已有 Codex/Claude/Kimi/GrokBot/Gemini/ZCode/WorkBuddy/DeepSeek 枚举与本地凭据发现（`quota.rs:129-146`），与 BrimDeck 的 provider 名单高度重合。**用量解析应复用同一 `Provider` 枚举**，让「额度」与「用量」在同一 provider 下并列。

**实测精确路径与格式（BrimDeck DataLocations.cs / 各 reader，第二份报告补充）**
| 工具 | 路径与格式 |
|---|---|
| Claude Code | `~/.claude/projects/**/*.jsonl`（JSONL，LogReader 增量解析） |
| Codex | `~/.codex/sessions+archived_sessions/*.jsonl`（JSONL）；额度来自 `auth.json` → `chatgpt.com/backend-api/wham/usage` |
| Kimi Code | `~/.kimi-code/sessions/**/agents/**/wire.jsonl`，`type=="usage.record"`，**每调用一条** |
| DSH（the agent harness） | `~/.dsh/sessions/<proj>/<sess>/session.vN.jsonl(.zstd)`，**zstd 帧按批**，取目录内最高版本 |
| ZCode | `~/.zcode/cli/db/db.sqlite` 表 `model_usage`（input 已含 cache） |
| OpenCode | `~/.local/share/opencode/opencode.db` 表 `session_message`(2.x)+`message`(1.x)，JSON 列；reasoning 计为 output |
| Antigravity | `~/.gemini/antigravity[-cli]/conversations/*.db`，从 `steps/gen_metadata` 十六进制**手工解 protobuf** |
| Cursor | `state.vscdb` 取 token → 调 Cursor API（按日分页事件） |
| Grok Bot | `%APPDATA%\Grok Bot\sand-secrets.json` + base32 命名的 `.blob` 缓存 |
| Pi | `~/.pi/agent/sessions`（仅用量） |
| Claude/GrokBot 本地额度 | `plan-usage-history.json` / `weekly-usage.cache` |

> **落地取舍**：岛仔若要覆盖多工具，优先做 **Claude Code / Codex / Kimi / ZCode / DSH / OpenCode**（JSONL 或 SQLite，开发量可控）；Cursor / Antigravity / Grok Bot 的私有格式（API 分页、手工 protobuf、加密 blob）工作量大，列为可选后置。**强烈建议借鉴 BrimDeck 的 script-provider 注册表机制**（内置 Jint JS 脚本跑各工具取数）：岛仔可用 Rust JS 运行时（boa/rquickjs）实现同款“不重新编译即可加工具”，且**额度指标（quota-metrics）与 token 台账（token-ledger）两条轴保持分离**。

**解析引擎（`LogReader.cs`）关键机制 —— 必须照搬**
- **增量解析**：每文件记住 `offset`（上次完整行末尾）+ 大小/修改时间；只解析新增行。文件变短或改写 → 从头重解析。`CacheItem{ Length, Offset, Modified, Entries }`。
- **快速过滤**：整行先查是否含 `"token_count"` / `"turn_context"` / `"usage"` 关键字（`HasUsageKey`），不含则跳过，避免逐行 JSON 解析大日志。
- **超长行丢弃**：>32MB 的单行（巨大 tool result）跳过并计为错误，不无限膨胀缓冲区。
- **去重合并**：Claude 跨文件重复的消息按 key 取 `total` 最大者；`if !entries.get(key) or entry.total >= old.total`。
- **剔除噪声**：`time < cutoff`（默认近 30 天）或 `total <= 0` 的记录丢弃。
- 岛仔落地：`src/ai_usage/logs.rs` 移植，纯逻辑可跨平台 + `#[cfg(test)]` 单测（坏行、去重、增量偏移都能脱离 Windows 测）。

#### 2.2.3 聚合与筛选（`ModelUsage.cs`）
```rust
struct UsageDateRange { start: Date, end: Date }           // 本地日；含末日整天
fn filter(snapshots, selected: HashSet<Provider>, range) -> ModelUsageResult
struct ModelUsageResult { rows: Vec<ModelUsageRow>, entries, incomplete_providers }
// rows 按 (provider, model) 分组，按 total 降序
```
- 日期快捷口径：**今日 / 近 7 日 / 近 30 日 / 自定义**（`UsageDateRange::Recent(1|7|30)`）。
- `incomplete_providers`：某工具未读到 / 记录不全 / 起始日落后于所选范围 → 标记“数据不全”，UI 显示 `≥` 或提示，不误报总量。

#### 2.2.4 UI（`UsagePanel.cs` + `UsageMetric` 渲染）
- **尺寸分层（Tier）**：Grand/Roomy/Default/Dense/Compact/Tiny 六档版式，字号/间距随面板高度切换（`UsagePanel.cs:36-44` 有精确数值表）。**这正是“功能随外形放大”的样板**（见 §9）。
- **多应用多列**：面板宽度足够时，多个工具（应用）并排为多列，每列可翻页显示其配额/用量；列宽 ≥150 DIP（`MinimumColumnWidth`）。
- **每个 metric 卡片**：标签行 + 大号数值（18–40pt 随档） + 用量条（Track） + 底部“今日/七日”双行合计（token 数与金额）。
- **模型明细页**：`ShowModelDetails` —— 按模型列 token 分解与成本，可逐项展开。
- 岛仔落地：`src/ai_usage_ui.rs`，初期可先用单列 + 简化 Tier（Default/Dense/Compact 三档），随面板尺寸开关自动切换；多列后置。

#### 2.2.5 刷新与同步（`UsageSynchronization.cs` / `UsageService.cs`）
- **60s 定时**（`DispatcherTimer`）；自动同步默认开；**单飞（single-flight）**：一个在跑时新请求排一个后续；各 provider 并行、**全局 90s 截止**；WSL 读取走独立线程按 key 合并；429 退避带 jitter。
- 本地日志增量解析（多数轮次无变化，`_merged` 缓存直接复用）；价格/额度为日级；磁盘缓存 `model-prices.json`、`claude-quota.json`、`kimi-quota.json`（最长 7 天有效）。
- **底部合计**：每应用页脚“今日/七日”两行 token 数 + 金额（`DashboardUsage`）。
- 岛仔落地：沿用 `quota::Service` 范式起 `ai_usage::Service`，`try_recv` 推快照；日志解析 CPU 密集、价格拉取网络 IO，分线程。

**模块划分**：`src/ai_usage/` 下 `mod.rs`（服务）、`logs.rs`（解析引擎）、`pricing.rs`（价格 + 反推）、`model.rs`（TokenEntry/UsageMetric/聚合）、`ui.rs`（面板）。
**UI 挂载**：与 P1-5 一样成为“可扩展页”（见 §5 改造）；**或**先在现有「AI 额度」页加子标签「用量/成本」，避免动页模型。收起态 HUD 显示当日 tokens/成本。
> **注意**：BrimDeck 原始设计中，**收起态只显示额度环，不显示 token/成本**（token/成本在展开页 `UsagePanel` 的每列底部“今日/七日”合计里，见 `MainWindow.CompactColumn.cs`）。岛仔若要“收起态显示 tokens/成本”，属超越 BrimDeck 的增强，需自定 HUD 版式（可放在现有 HUD 一行，如 `AI 12.3k · $0.21`）。
**Settings**：`ai_usage_enabled`、`ai_usage_providers:[bool;N]`、`ai_usage_period`、`ai_usage_hud`（显示 tokens/成本/不显示）、`ai_usage_pricing_source`（联网目录/仅本地）。

**风险**：各工具记录格式随版本变 → 解析层强容错 + 单测覆盖真实样本；价格目录联网依赖（离线可退本地价/反推）。BrimDeck 已大规模实测这些格式，移植风险远比“从零设计”低。

---

### P0-3 快捷翻译（全局快捷键）

**目标**：全局快捷键唤起独立翻译窗口（中英自动识别/中译英/英译中），Tab 一键填入最新剪贴板，译后一键复制。后端复用 DeepSeek API，密钥存 Windows 凭据管理器。

**参考**：fly-island `translation.rs` 全流程（`RegisterHotKey MOD_NOREPEAT` + `WM_HOTKEY` 消息窗口线程；keyring；DeepSeek chat completions）。

**模块**：`src/translate.rs`（快捷键线程 + 服务调用 + keyring），`src/translate_ui.rs`（独立 viewport 窗口，复用 `show_viewport_immediate` 范式如设置窗口）。
**实现要点**
- 全局热键默认 `Alt+C`，可配置；用 `RegisterHotKey` 于专用消息窗口线程（岛仔 `input.rs` 已有 Win32 消息处理经验可复用）。
- 窗口：输入框 + 目标语言切换（自动/中→英/英→中）+ 结果区；Tab 从 `clipboard::Shared` 取最新文字填入；结果一键复制（`Cmd::Copy`）。
- DeepSeek 调用：`POST https://api.deepseek.com/chat/completions`，模型沿用现有 `deepseek_quota` 连接能力与代理（`quota::http_agent` 已有系统代理逻辑，抽取复用）。超时/错误友好提示。字符上限 5000（对齐 fly-island）。
- 密钥：`keyring` entry `("com.daozai.translate","deepseek")`，与用户已有的 DeepSeek 额度凭据可考虑共用（`deepseek_quota.rs` 已用 keyring，实现期确认 service 名）。

**Settings**：`translate_enabled`、`translate_hotkey`（存字符串）、`translate_auto_copy`。

---

## 3. P1 — 效率与监控增强

### P1-4 图文剪贴板升级

**目标**：文字扩展为“文本 + 图片”混合历史，加搜索/备注/收藏；历史数量可配；敏感内容（密码管理器标记）自动排除。

**现状**：`src/clipboard.rs` 仅文字，已有排除逻辑（`ExcludeClipboardContentFromMonitorProcessing` 等）、去重、固定。

**改造**
- `Clip` 增加 `kind: Text|Image`、`note: Option<String>`、图片以 PNG 存 `%APPDATA%\Daozai\clipboard-blobs\<hash>.png`（去重），`clipboard.json` 只存引用 + 缩略图元数据。
- 监听器读 `CF_DIB/CF_BITMAP`（或 `CF_PNG`）→ `image` crate 编码落盘。注意大图/上限（沿用 `MAX_CHARS` 的对等像素上限）。
- UI（置物架页 `shelf_tab==1` 剪贴板分栏）：搜索框（按文字/备注）、备注编辑、收藏（复用 `pinned`）、图片缩略图行。
- `Settings`：`clipboard_max_recent`（可配，替代常量 `MAX_RECENT=30`）。

**验收**：截图复制后出现在历史；密码管理器内容不进历史；搜索/备注/收藏持久化。

### P1-5 系统监控面板（借鉴 BrimDeck）

**目标**：电源/网络/音量之外，增加 CPU/GPU/内存/磁盘实时读数；收起态岛体边缘极简数字或环形指示；可选温度/功耗，硬件传感器兼容性差则静默跳过。

**模块**：`src/sysmon.rs`（采样服务，`#[cfg(windows)]` 实现，非 Windows 返回空快照占位），`src/sysmon_ui.rs`（面板）。

**核心模式（移植 BrimDeck 思想）**
```rust
enum Reading { Known(f32), Unknown(&'static str) } // 永不伪造 0
struct Snap { cpu: Reading, gpu: Reading, mem_used: Reading, mem_total: Reading, disk: Vec<DiskReading>, cpu_temp: Reading, gpu_temp: Reading, power: Reading, at: i64 }
```
- 采样线程按需启动（面板打开/收起态 HUD 开启时），BelowNormal，1–2s 间隔；不可读项 `Unknown`。
- 指标实现（Windows，无管理员）：
  - CPU%：`NtQuerySystemInformation(SystemProcessorPerformanceInformation)`（单 syscall，无需 PDH 服务）
  - GPU%：PDH `\GPU Engine(*)\Utilization Percentage`（`windows` crate `Pdh*`）
  - 内存：`GlobalMemoryStatusEx`；磁盘：`GetDiskFreeSpaceEx`（每卷）
  - 温度：ACPI `\Thermal Zone Information(*)`（PDH）；GPU 温度 `D3DKMTQueryAdapterInfo` —— **不可读即 Unknown**
  - 功耗：`\Energy Meter(*)\Power`（PDH，普通权限）；逐核 MSR 功耗需驱动 → **不在首版范围，静默跳过**
- 传感器兼容：所有 PDH/driver 调用加超时/退避；异常值（0/负/>125℃）丢弃为 `Unknown`；不装内核驱动、不申请管理员。
- **页面改造**：本条需成为可切换的第 9 页。改造方案见 §5（`visible_pages` 等数组从 8 扩到 9，逐处修正下标映射）。
- 收起态：极简数字（如 `C 12%  G 3%  M 48%`）或 ¾ 圆环，作为岛体边缘 HUD 元素（复用现有 `HUD`）。
- `Settings`：`sysmon_enabled`、`sysmon_page`（是否作为页）、`sysmon_hud`（收起态显示项位掩码）、`sysmon_sensors`（温度/功耗开关）。

---

## 4. P2 — 可扩展性与体验打磨

### P2-6 消息通知智能聚合

**目标**：在微信/QQ/邮箱基础上：免打扰时段、按应用静音、通知摘要（“5 条微信未读”折叠）；接入飞书/钉钉（与日历形成办公闭环）。

**现状**：`src/system.rs`（Windows 通知来源）、`src/wechat.rs`、`src/qq_toast_source.rs`、`src/mail.rs`、`src/app.rs` 未读聚合（`Unread`/`UNREAD_*`）。
**改造**
- `Settings` 新增 `dnd_start/dnd_end`（免打扰时段，期间静音不闪但收进摘要）、`mute_apps: Vec<String>`、`notify_digest: bool`（折叠显示）。
- UI：收起态聚合为“N 条未读”；展开态按来源分组折叠。
- 飞书/钉钉接入：调研官方开放平台/本地通知通道；**风险高（需 OAuth 或第三方），列为独立可选子任务**，先做“本地 Windows 通知来源是否包含飞书/钉钉”的可行性验证。

---

## 5. 关键技术决策：从 8 页扩到 9 页

新增“系统监控”需要第 9 页。现有 `visible_pages:[bool;8]`、`page_order:[usize;8]`、`PAGE_NAMES:[&str;8]`、`[1,2,3,5,4,7,11,8]` 映射、`settings.txt` 中 `visible_pages` 变长字符串兼容、`bar_layout::valid_order`、`navigate_page` 的 `PAGE_NAMES.len()` 等都隐含 8。**统一改造清单**：
1. `model.rs`：`visible_pages`/`page_order` 改 `[..;9]`（默认 `[true;9]`，`page_order` 补 8）；`load`/`save` 的 `visible_pages` 变长兼容（旧 8 位补位）。
2. `app.rs`：`PAGE_NAMES` 加 `"系统"`；`render_page` 加 `8 => self.page_sysmon(c)`；`[1,2,3,5,4,7,11,8]` → 补第 9 项设置 tab 映射。
3. `extras.rs`：`settings_tab` 新增「系统监控」页；`titles`/`SETTINGS_GROUPS`/`descriptions` 同步。
4. `bar_layout.rs`：`valid_order` 上限 8→9；`ui_tests.rs` 相关断言更新。

> 若无必要，P1-5 也可先只做“收起态 HUD + 设置页里的展开面板”，避免动页模型；是否成页在评审时定。**推荐：先 HUD + 设置内面板，稳态后再决定是否提为第 9 页**（降低回归面）。

---

## 6. 建议的实施顺序与里程碑

| 里程碑 | 内容 | 依赖 | 估算 |
|---|---|---|---|
| M0 准备 | 扩 `Settings` 骨架字段；确认 keyring service 名；数据勘察 spike（BrimDeck 用量格式照搬核对、Qoder/WorkBuddy/Trae hook 能力、飞书/钉钉通知可行性） | — | 2d |
| **M1 (外形放大)** | 在 `Card` 层加版式缩放钩子；`PW/PH` 等比放大 + `island_size` 上限提到 160%；更新 `ui_tests.rs` 快照基线。**先把画布做大，后续功能才有空间** | — | 5d |
| M2 (P0-3) | 快捷翻译：热键 + 窗口 + DeepSeek + keyring。**最小闭环、风险最低，先落地验证服务/热键基建** | M0 | 4d |
| M3 (P0-1) | 状态灯：标记/状态读取 + Codex/Claude 安装器 + 收起态 + 面板；再扩 Qoder/WB/Trae | M1 | 6d |
| M4 (P0-2) | 用量/成本（移植 BrimDeck）：`logs.rs` 解析引擎 + `pricing.rs`（OpenRouter 目录+手动价+反推）+ 聚合 + 面板 + HUD | M1, M0 spike | 8d |
| M5 (P1-4) | 图文剪贴板：图片监听 + 存储 + 搜索/备注/收藏 + 数量配置 | M1 | 4d |
| M6 (P1-5) | 系统监控：采样服务 + HUD + 面板 + 传感器降级（多档 Tier） | M1, M0 | 6d |
| M7 (P1-5→Step3) | 尺寸预设（紧凑/标准/宽敞）+ 可拖拽 + 用量/监控页 Tier 精修 | M1, M4, M6 | 5d |
| M8 (P2-6) | 通知聚合：免打扰/静音/摘要；飞书/钉钉（按可行性降级） | M1 | 5d+ |

每个里程碑结束**必做**：`cargo build`（含 `--features record`）、`cargo test`（`ui_tests.rs` 大量快照）、录制器手工验证、更新 `CHANGELOG.md` 与 `docs/USAGE.md`。

---

## 7. 风险与待确认项

1. **AI 用量数据源**（P0-2）：各工具本地格式不公开、版本易变 → 先做数据勘察 spike；解析器强容错；无法识别的工具标记“暂不支持”。
2. **Qoder / WorkBuddy / Trae 是否提供 hook**（P0-1）：需实测；无 hook 则该灯不可用，绝不退回进程探测（会有误判，fly-island 已明确反对）。WorkBuddy 岛仔已有额度连接，可能可复用其认证态但非 hook。
3. **GPU% / 温度 / 功耗的硬件兼容性**（P1-5）：PDH 计数器依赖性能监视器服务；部分虚机/精简系统无 `\GPU Engine`；`D3DKMT` 温度个别驱动会给 0 → 一律 `Unknown`，`—` 显示。
4. **飞书/钉钉接入**（P2-6）：可能需要 OAuth/第三方库，工作量大，建议单独立项。
5. **8→9 页改造**（§5）：回归面提示，优先用 HUD/子面板规避。
6. **DeepSeek 密钥复用**：确认与 `deepseek_quota` 的 keyring service 名一致，避免用户重复录入。
7. **Hook 安装器对用户配置文件的写入**：必须幂等、可卸载、块包裹、备份；写入前校验格式，失败不破坏原文件（fly-island 的 `write_text_file` 先写 `.tmp` 再 rename 原子替换，值得照搬）。
8. **外形放大的回归面（新增）**：`pages.rs` 全页硬编码坐标基于 419×140，`ui_tests.rs`（382KB 快照）会大面积失效。**对策**：不在页内逐坐标改，而在 `paint::Card` 集中坐标入口加缩放钩子（`pos/rect/text/fit` 统一 ×k），先等比放大；快照基线按新尺寸重录。放大会牵动 `native.rs` 的窗口命中/毛玻璃轮廓、`shape.rs` 圆角肩部、日记/邮箱详情页高度等，需逐项验证遮挡与穿透。
9. **收起态放大取舍（新增）**：收起态过大会挡桌面。放大主战场是展开面板；收起态仅做克制的宽高微增（或照 BrimDeck 用“高度驱动内容 scale”而非大幅加宽）。
10. **`PanelScale` 语义**：BrimDeck 仅 100%/150% 两档；岛仔若要自由百分比属超出参考、需自测各档无重叠/无裁切。

---

## 8. 代码落点速查（预计新增/改动）

**新增**
- `src/agent_status.rs`（+ 可选 `src/agent_status/{codex,claude,qoder,workbuddy,trae}.rs`、`scripts/daozai-agent-*.{cmd,ps1}`）
- `src/ai_usage/`（`mod.rs` 服务、`logs.rs` 解析引擎、`pricing.rs` 价格+反推、`model.rs` TokenEntry/UsageMetric/聚合、`ui.rs` 面板）
- `src/translate.rs`、`src/translate_ui.rs`
- `src/sysmon.rs`、`src/sysmon_ui.rs`
- `assets/pricing/`（内置兜底价：DeepSeek/Kimi/智谱等官方价；OpenRouter 目录联网获取并缓存）
- `src/bin/` 无需新增

**改动**
- `src/model.rs`：`Settings` 新增各开关字段 + `load`/`save`；`ISLAND_SIZE_MAX` 140→160；新增 `panel_size`
- `src/app.rs`：`PAGE_NAMES`、`render_page`、（若成页）页数组、收起态 HUD；`PW/PH/WIN_W/WIN_H` 基础尺寸放大；`expanded_height` 各页等比
- `src/paint.rs`：`Card` 层加“版式缩放因子”钩子（`pos/rect/text/fit` 统一 ×k）——**放大改造的核心落点，改动面最小**
- `src/extras.rs`：设置页新增分组/页；`titles`/`SETTINGS_GROUPS`
- `src/lib.rs`：注册新模块
- `src/clipboard.rs`：图片/备注/搜索/数量配置
- `src/main.rs`：启动各服务线程（`*::Service::start()` 或 `*::watch()`），热键初始化
- `Cargo.toml`：按需加 `windows` feature（`Pdh`、`System_Performance`、`System_ProcessStatus`）、必要时 `sysinfo`；`image` 已可编码 PNG
- `src/ui_tests.rs`：快照基线随放大重录
- `CHANGELOG.md`、`docs/USAGE.md`

> 所有参考代码已就位于 `E:\github\_refs\`，实现阶段可逐一对照。**当前仅完成计划，未改动任何代码。**

---

## 9. 外形放大：对标 BrimDeck 的尺寸与版式

> 需求调整：把岛仔外形参考 BrimDeck 放大，并让功能随之合理放大。

### 9.1 差距量化（BrimDeck 实测 vs 岛仔现状）

| 维度 | BrimDeck（DIP） | 岛仔现状（逻辑点） | 备注 |
|---|---|---|---|
| 收起态 · 药丸/胶囊 | **196×32**（CapsuleWidth/Height 默认） | 163×30 | 宽接近，但 BrimDeck 高度可 24–64、内容按高度缩放 |
| 收起态 · 刘海 | **238×32** 默认，高 24–64 | 刘海沿用 163×30 岛体 | BrimDeck 高默认 32、可到 64 |
| 收起态极简线 | 56×4 | 44×4.5 | 接近 |
| 展开面板 · 最小值 | **440×140** | 419×140 | 岛仔宽度略低于 BrimDeck 下限 |
| 展开面板 · 默认（监控页） | **620×220** | 419×140 | 岛仔明显小；用量页默认 **520×200** |
| 展开面板 · 用量页 1 应用 | 500×170 / 550×200 / 630×220（紧凑/标准/宽敞） | — | 随应用数与预设变化 |
| 展开面板 · 用量页 4 应用 · 宽敞 | **1100×310** | — | 多列并排 |
| 展开面板 · 最大值 | **1200×400**（×PanelScale，150% 即 1800×600） | 419×140 固定 | |
| 全局缩放 | `PanelScale` 仅两档 **100%/150%**（整档等比） | `island_size` 80–140% 自由 viewport scale | 机制相近；岛仔反而更灵活 |
| 面板尺寸预设 | 紧凑/标准/宽敞 三档 + 可按页拖拽 | 无（固定常量） | |
| 版式分层 | Grand/Roomy/Default/Dense/Compact/Tiny 六档随高度切换 | 无（单一尺寸） | §2.2.4 已引用 |

**结论**：岛仔的收起态尺寸与 BrimDeck 接近；差距主要在**展开面板**（419×140 vs 默认 620×220、上限 1200×400）与**缺少尺寸预设/多档版式**。功能（尤其用量/系统监控）需要的画布比现有 419×140 大得多。

### 9.2 放大方案（分三步，风险递增）

**Step 1 · 提升基础面板尺寸与上下限（低风险）**
- 在 `app.rs` 常量把 `PW/PH` 由 `419×140` 提升到接近 BrimDeck 标准档，如 **`PW=560, PH=200`**（可先 520×190 过渡）；`WIN_W/WIN_H/CANVAS_W` 同步放大（如 `WIN_W=780, WIN_H=360`）。
- `expanded_height()` 各页覆写（日程 `CALENDAR_H=216`、邮箱详情 236）等比增大。
- 同批把 `island_size` 上限从 140% 提到 **160%**（`ISLAND_SIZE_MAX`），让用户仍可微调。
- 影响面：**所有绘制坐标基于 419×140 局部逻辑点**（`pages.rs` 里大量硬编码如 `44.0, 13.0, 121.0...`）。所以不能只改常量——见 Step 2 的“缩放层”策略。

**Step 2 · 引入“版式缩放因子”而非逐坐标重写（中风险，推荐）**
- 不逐个改页内硬编码坐标，而是给每页绘制传入一个 `layout_scale = PH_new / 140`（及宽度因子），在 `Card` 层的 `pos()/rect()/text()/fit()` 统一乘算。`paint::Card` 已是集中坐标入口（`pages.rs` 全程用 `c.*`），**在这里加缩放钩子改动面最小**。
  - 先做“等比统一缩放”：`PW/PH` 放大 k 倍，`Card` 所有尺寸×k，字号×k。快速得到“整体变大”，视觉与现有完全一致，只是更大。
  - 再针对个别页面（音乐封面、额度环、日历格）做“非等比”精修（宽度多留白、高度多给行距），接近 BrimDeck 的 Roomy/Grand 观感。
- 单元/快照测试 `ui_tests.rs`（382KB）会大量受影响：需按新尺寸更新基线快照，或让测试以缩放因子参数化。

**Step 3 · 尺寸预设 + 多档版式（体验打磨，可后置）**
- 对标 BrimDeck：新增 `panel_size: Compact|Standard|Spacious` 设置（映射到不同 `PW/PH` 或 k 值），并把 `PanelScale`（百分比整体缩放）与 `island_size` 合并/并存说明清楚。
- 面板可拖拽边调整大小（对标 `DeckPages::SetSize` 的 clamp 到 Min/Max）。
- 用量/系统监控页实现 Tier 分层（按面板高度选字号/间距档），达成“功能随外形放大”。

### 9.3 BrimDeck 外形的关键机制（实读，直接影响移植）
- **收起态是“高度驱动”的**：以 32 DIP 为基准高，所有内容尺寸 × `Scale = surfaceHeight / 32`（高 24→scale 0.75，高 64→scale 2.0）。**加宽只拉长中间内容（环/读数/歌词），不会放大各部件**。岛仔若照此，收起态放大应主要调高度而非宽度。
- **收起态是“单行轮播”**，不是固定 N 列网格：cover + N 个额度环 + 滚动歌词/标题 + 均衡器 + 时钟 + 系统读数条，全部塞进一条 ~24 DIP 的行里（表面 32 DIP，上下各约 4 DIP 内边距）；太窄时按 `LeaveOrder` 依次丢项。岛仔收起态 HUD 设计可借鉴“轮播 + 优先级丢项”。
- **`PanelScale` 实际只有两档：100%（默认）/ 150%**（`Choices=[100,150]`，其它值 snap 回 100），**是整档等比放大**（收起+展开+字体+圆角一起 ×1.5），不是自由 80–140% 滑条。→ 岛仔若要“自由百分比”，属于**超出 BrimDeck**，需自行扩展；可保留 `island_size` 的 80–160% 作为自由档，另加一个“150% 大屏模式”一键。
- **面板尺寸预设（未缩放 DIP）**：用量页 紧凑500×170 / 标准550×200 / 宽敞630×220（1 应用）、最多 1100×310（4 应用）；监控页 紧凑480×160 / 标准620×220 / 宽敞860×320；音乐页 440×140 / 440×200 / 760×300。**全局页尺寸上下限 440–1200 × 140–400**（×scale，150% 时最大 1800×600）。
- **收起态各部件（scale=1）**：行高 24、封面 20(r4)、均衡器 18×14、额度环 ⌀12、系统表盘 ⌀16、主字 11.5 / 副字 8.5、部件间距 8、读数间距 6。
- **展开态**：圆角 24；表盘 ⌀19–26（随字号 16/19/23）；字号随页高 16–23/11–12/10–10.5。
- **阴影**：BlurRadius 14 / 深度 4 / 向下 / 不透明 0.28；面板两侧各留 12 DIP、抓手 14 DIP。

### 9.4 收敛建议

- **岛仔现状**：`island_size` 是 80–140% 自由档（viewport scale），比 BrimDeck 的两档更灵活；**收起态药丸 163×30 与 BrimDeck 胶囊 196×32 量级相当**。真正差距只在**展开面板**（419×140 vs 标准 520–620 宽、上限 1200×400）与**缺少尺寸预设/多档 Tier**。
- **先做“展开面板等比放大 k≈1.35–1.5”**（`PW 419→~565, PH 140→~190`；`WIN_W/WIN_H` 同步），在 `paint::Card` 层加缩放钩子（所有页坐标都走 `c.*`，改动面最小），一次性整体变大且页内容视觉不变。
- **收起态**：药丸/刘海略放大（`BAR_W 163→~190, BAR_H 30→~34`），但这会挡桌面——**收起态放大要克制**，主要空间给展开面板；可学 BrimDeck 用“高度”微增触发内容 scale，而非大幅加宽。
- **再加尺寸预设**：新增 `panel_size: Compact|Standard|Spacious`（映射到不同 `PW/PH`），并把 `island_size`（自由 80–160%）与 `panel_size` 并存；用量/监控页做多档 Tier（按面板高度切换字号/间距）。
- **按页独立尺寸**：BrimDeck 每页有独立宽高预设，岛仔现为统一 `PW/PH` + 个别页覆写高度；可扩展为每页独立宽高。
- **可拖拽调整**：对标 `DeckPages.Limits`（440×140 ~ 1200×400，×scale）clamp。

### 9.5 与 BrimDeck 的契合点清单
- 面板尺寸预设 紧凑/标准/宽敞 + 两档整体缩放（100/150%）—— 岛仔可映射为 `panel_size` + `island_size`。
- 六档 Tier 版式（Grand/Roomy/Default/Dense/Compact/Tiny）—— 用量页/监控页复用，避免“放大后空旷”。
- 按页独立宽高 + 可拖拽 + Min/Max clamp。
- 收起态“高度驱动内容缩放 + 单行轮播 + 按优先级丢项”。
