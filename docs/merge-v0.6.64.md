# 合并上游 v0.6.64（commit 6eb3373）

本文件记录把上游 `Renly1994/xiaotang-daozai` 的 v0.6.64 合入本 fork `mine` 分支的实际过程：
合并对象、指导原则、冲突规模、处理策略、结构改动、编译期踩到的问题、测试策略与回滚办法。

> 配套阅读：
>
> - [fork-sync-notes.md](fork-sync-notes.md) —— 同步守则、冲突规则、必须停下来问的情形
> - [merge-history.md](merge-history.md) —— 第一次同步的完整操作流水与分支演化

---

## 一、合并对象

| 项 | 值 |
|---|---|
| 上游提交 | `upstream/main` `6eb3373`（v0.6.64） |
| 上游提交规模 | 60 个非合并提交 |
| mine 侧基线 | `54c53db` |
| 本仓库 origin / upstream | `weiln/daozai` / `Renly1994/xiaotang-daozai` |
| 合并方向 | `mine` ← `upstream/main` |

---

## 二、指导原则

本次合并由用户明确两条原则，冲突一律按这两条裁决：

> 1. 「如果上游有的功能，就把我自己开发的去掉，尽量跟上游保持一致」
> 2. 「先处理冲突，尽量减少对上游代码的入侵」

由此推出的三条落地规则：

1. **上游有等价实现 → 删自己的**。用户私有实现只在上游没有对应能力时才保留，避免同一功能维护两套。
2. **优先整块采用上游**。冲突块里不确定语义时，整段取上游，再用最小的追加改动把私有能力接回去。
3. **私有能力以追加为主，不改上游既有分支**。新增枚举项放末尾、新增分支加在已有分支之后，减少下次合并的冲突面。

---

## 三、冲突规模

合并共产生 **16 个冲突文件、100 个冲突块**。

| 文件 | 冲突块 |
|---|---|
| `CHANGELOG.md` | 1 |
| `Cargo.lock` | 1 |
| `Cargo.toml` | 2 |
| `app.rc` | 2 |
| `src/app.rs` | 23 |
| `src/bar_layout.rs` | 9 |
| `src/clipboard.rs` | 1 |
| `src/extras.rs` | 8 |
| `src/main.rs` | 5 |
| `src/model.rs` | 7 |
| `src/quota.rs` | 6 |
| `src/quota_ui.rs` | 11 |
| `src/resident_settings.rs` | 3 |
| `src/system.rs` | 1 |
| `src/ui_tests.rs` | 15 |
| `src/wechat.rs` | 5 |
| **合计** | **100** |

冲突集中在 `src/app.rs`（23）、`src/ui_tests.rs`（15）、`src/quota_ui.rs`（11）三处，
它们分别对应「外壳与页面装配」「界面测试」「额度界面」——正是上游 v0.6.64 改动最密集、
本 fork 私有改动也最密集的三个面。

---

## 四、处理策略

### 4.1 整文件采用上游

以下文件整体取上游版本，本 fork 的改动不保留：

| 文件 / 段落 | 原因 |
|---|---|
| `src/wechat.rs` | 上游重写了微信提醒与唤起流程，私有版本基于旧的采集通道 |
| `src/resident_settings.rs` | 上游重写常驻设置模型，私有版本依赖系统监控组与 1.4 倍缩放 |
| `src/bar_layout.rs` | 上游重写常驻栏布局（天气进常驻栏、9 页顺序），私有版本按 10 页与 1.4 倍基准排布 |
| `src/extras.rs` 的 settings_ui 整段 | 设置页整段装配，冲突块互相依赖，逐块拼合反复出错（见第五节） |

### 4.2 上游有等价实现 → 直接删除自己的

| 删除 | 改用上游 |
|---|---|
| `src/messenger_flash.rs` | `src/messenger.rs`：`enum Messenger { Wechat, Qq, WeCom, Feishu, DingTalk }` + 统一 Adapter 表 |
| `src/feature_pages.rs` | 上游页面装配（`src/app.rs` + 页面模块） |
| `src/qq_toast_source.rs` | 上游 `src/toast_source.rs` |
| 微信私有实现（旧 `src/wechat.rs`） | 上游重写后的 `src/wechat.rs` |
| `src/notes.rs` / `src/notes_ui.rs` | 无（便签整体下线，`Alt+C` 不再绑定窗口） |
| `src/sysmon.rs` | 无（上游 v0.6.64 已完全移除系统监控，`git grep` 为空） |
| `src/notify_policy.rs` | 上游统一消息提醒的去重与首轮提醒规则 |
| `assets/wechat-client.png`、`assets/dingtalk-client.png`、`assets/dingtalk-logo.png` | `assets/calendar/dingtalk.svg` 等上游资源 |

### 4.3 上游没有 → 最小侵入保留

| 保留项 | 侵入方式 |
|---|---|
| Trae CN 额度（`src/trae_quota.rs`、`Provider::TraeCN`） | `Provider` 枚举在**末尾追加**：上游 16 项 + TraeCN = 17 项；`quota_ui` 里只加 Trae 分支，不改已有分支 |
| 图文剪贴板（`Clip` 的 `image` / `kind` / `pinned`、CF_DIB→PNG、缩略图与悬停预览） | 以追加字段与独立绘制函数实现，`src/clipboard.rs` 冲突块沿用上游骨架 |
| NO_PROXY 内网直连旁路 `http_agent_for(target)` | 作为独立函数提供，调用点替换保持一行，不改上游请求构造 |
| `scripts/sync-upstream.ps1`、`docs/fork-sync-notes.md`、`docs/merge-history.md`、`.archify` 忽略 | 纯 fork 侧文件，上游无对应物，冲突为零 |

---

## 五、结构冲突说明

上游把「页面数」从本 fork 的 10 页收回 9 页，并新增天气页，导致页面索引整体变化：

- `visible_pages` / `page_order` 的元素类型从本 fork 的 `[bool; 10]` 改为上游的 `[bool; 9]`。
- 天气页占 **index 8**（9 页顺序：音乐 / 专注 / 健康 / 置物架 / 待办 / 邮箱 / AI 额度 / 日程 / 天气）。
- `PAGE_NAMES` 为 **9 项**，与 `[bool; 9]` 一一对应。
- `PAGE_SETTINGS_TABS = [1, 2, 3, 5, 4, 7, 11, 8, 12]`，即每一页对应的设置分页索引。

私有页面（系统监控、AI 用量）下线后，旧配置里的这些页索引不再有意义：布尔数组按上游 9 项重新解释，
越界的旧位直接丢弃，不影响其余页的开关与顺序。

几何基准也随之回到上游：`PW = 419` / `PH = 140`，取消 `LAYOUT_SCALE = 1.4`。

---

## 六、编译期踩到的关键问题与结论

| 问题 | 结论 |
|---|---|
| `ENV_LOCK` 原本是 `ui_tests` 里的私有静态量，合并后额度相关测试也要用 | 上移到 lib 层公开，`ui_tests` 与其它测试模块共用同一把锁 |
| `src/system.rs` 合并时丢了 `Notifications` 与 `ToastVersion` 类型，导致引用处成片报错 | 从上游版本补回这两个类型；该文件冲突只有 1 块，但丢失的是块外类型定义 |
| `src/app.rs` 少一个闭括号，其后 15 个方法掉出 `impl` 块，报成「方法不存在」 | 修括号即可，症状与原因相距很远，先看 `impl` 边界再看方法体 |
| `src/extras.rs` 的 settings_ui 整段被反复改坏 | 停止逐块拼合，整段采用上游，再在其上做追加改动 |
| `src/clipboard.rs` 的 `run()` 换成上游新签名 | 新签名需要 `class` 常量，调用点按上游签名补齐参数 |
| `src/app.rs` 紧凑栏漏画常驻倒计时 | 按上游紧凑栏绘制顺序补回倒计时绘制 |

---

## 七、测试策略

`ENV_LOCK` 用的是 `std::sync::Mutex`，**不可重入**：同一线程重复加锁会自死锁。
因此锁只能加在不调用 `H::new()` 的测试辅助函数上：

- 加锁：`doubao` 的 `with_data_dir`（改动环境变量 `data dir` 的辅助函数）
- 不加锁：`H::new()` 及其调用链（它内部会走到 `with_data_dir`，外层再加锁就会在同一个线程上二次加锁）

---

## 八、结果

| 检查 | 结果 |
|---|---|
| `cargo check --tests` | 0 error |
| `cargo test` | 1090 passed / 0 failed / 115 ignored |

---

## 九、回滚办法

按「离出错点越近越优先」的顺序使用：

1. **合并提交尚未产生时**：`git merge --abort`，工作区与索引回到合并前。
2. **已产生合并提交、想退回备份分支**：

   ```
   backup/main-before-sync        = e124787   # 最原始的 main（拆分前的混杂状态）
   backup/mine-before-linear-20261111 = 427095d   # 历史改写前的回滚点
   ```

   切回 `mine` 的旧状态：`git reset --hard 427095d`（确认当前分支是 `mine` 且工作区无需保留的内容后再执行）。

---

## 十、下次同步的提醒

- 本次把私有功能整体让位给上游，冲突面会显著变小；剩余的私有改动只有 `trae_quota`、剪贴板图文、
  `http_agent_for` 与 fork 侧脚本 / 文档，都应以「追加」形式存在。
- 页面数是新的冲突高发点：只要上游再增删页面，`[bool; N]`、`PAGE_NAMES`、
  `PAGE_SETTINGS_TABS` 三处必须同时改，漏一处就会静默错位。
- 编解码与写文件仍按 [fork-sync-notes.md](fork-sync-notes.md) 的约定：UTF-8 无 BOM、LF。
