# Fork 合并上游：完整操作流水（2026-11-08 ～ 2026-11-11）

本文件是 `docs/fork-sync-notes.md` 的**配套实录**：那份是"守则与坑"，这份是"按时间顺序到底做了什么、每个 commit 从哪来到哪去"。
新开会话要看同步流程，**两份一起读**。

- 守则 / 冲突规则 / 编码坑 → [fork-sync-notes.md](fork-sync-notes.md)
- 操作流水 / 分支演化 / 失误复盘 → 本文

---

## 一、终局状态（2026-11-11 14:07 核对）

```
* 427095d (mine, origin/mine, backup/mine-before-linear-20261111) 同步脚本增加 main 纯镜像前置守卫…
*   96b4044 合并上游 f046d6c：腾讯企业邮箱尾空格解析、HTTP 代理下 HTTPS 请求格式
|\
| *   f046d6c (upstream/main, origin/main, main) 合并 PR #95：修复 HTTP 代理下的 HTTPS 请求格式
| |\
| | * 33947cd fix: 修复 HTTP 代理下 Grok Bot 额度查询 403
| | * 241ff1b vendor: 导入 ureq 2.12.1 源码
| * |   19caaf0 合并 PR #94：兼容腾讯企业邮箱能力列表末尾空格
| | * 6595ac7 fix(邮箱): 兼容腾讯企业邮箱能力列表末尾空格
* | 2feb8a9 系统监控页标题改为性能监控
* | f5b5950 AI 额度新增 Trae CN：读取浏览器令牌显示剩余积分
* | dc56be9 便签替换快捷翻译：版式对齐「新建日程」表单
* | a34eae7 同步守则新增最高优先级规则：冲突拿不准必须停下来问
* | 3a7a163 补充 fork 同步上游的踩坑记录
* | f989030 翻译与额度请求按目标选择代理
* | f1d48af 同步脚本改用 UTF-8 BOM
* | 17cae2f 新增上游同步脚本
* | 2936618 合入上游 v0.6.63：适配缩放布局、清理重复监听代码
* | 6995b60 修复翻译窗口深色主题、监控环占位
* | 31fa969 提交快捷翻译自定义接口、剪贴板缩略图与系统监控读数勾选
* | c834037 修复钉钉提醒丢未读、图标不显示与二次提醒延迟
* | bf7e61f 钉钉闪动结束会收掉未读
```

| ref | commit | 含义 |
|---|---|---|
| `upstream/main` | `f046d6c` | 上游最新 |
| `main` | `f046d6c` | 纯上游镜像（`git merge-base --is-ancestor upstream/main main` = 0） |
| `origin/main` | `f046d6c` | 已推送，ahead/behind = 0/0 |
| `mine` | `427095d` | 工作分支，领先上游 18 个提交，落后 0 |
| `origin/mine` | `427095d` | 与本地一致 |
| `backup/main-before-sync` | `e124787` | **最原始状态**备份（拆分前的 main） |
| `backup/mine-before-linear-20261111` | `427095d` | 历史改写前的回滚点 |

---

## 二、阶段 0：问题诊断（起点）

| 项 | 初始状态 |
|---|---|
| 远程 | 只有 `origin` → `weiln/daozai`（自己的 fork） |
| 本地 `main` | `e124787`，比上游旧版本**领先 7 个自己的 commit** |
| upstream | **未配置**，拉不到作者新版本 |
| 隐患 | 个人提交直接压在 `main` 上，与上游基线混杂，以后每次 pull 都会大范围冲突 |

**核心问题**：私有提交和上游基线共用一条分支，没有隔离。

---

## 三、阶段 1：确立分支模型（2026-11-08）

```powershell
git remote add upstream https://github.com/Renly1994/xiaotang-daozai.git
git remote set-url --push upstream DISABLE    # 防止误推上游
git fetch upstream
```

```
upstream/main  Renly1994/xiaotang-daozai   ← 新版本来源，push=DISABLE
origin         weiln/daozai                ← 自己的 fork
main           pure upstream mirror        ← 只许 ff，永不提交
mine           私有提交全在这里            ← 永远 rebase 到 main
backup/*       动手前的安全快照
```

---

## 四、阶段 2：一次性历史拆分

```powershell
git branch backup/main-before-sync        # 双保险：分支
git tag backup-before-sync                # 双保险：tag
git branch mine                           # mine 拿走全部私有提交
git checkout main
git reset --hard upstream/main            # main → 39413b3（纯上游）
git checkout mine
git rebase upstream/main                  # 7 个提交重放到上游之上
```

### 这次 rebase 的冲突面

**18 个文件双方都改过**，其中高频文件：

`src/app.rs`、`src/wechat.rs`、`src/model.rs`、`src/main.rs`、`src/pages.rs`、
`src/ui_tests.rs`、`Cargo.toml`、`Cargo.lock`、`CHANGELOG.md`、`app.rc`

### 冲突处理结论速查

| 文件 | 冲突本质 | 处理 |
|---|---|---|
| `Cargo.lock` | 依赖树 | `git checkout --theirs` 后 `cargo build --locked` 重建 |
| `Cargo.toml` | 版本号 | 取上游版本号，自己的依赖段保留 |
| `CHANGELOG.md` | 版本段落 | 新版在上：自己的 v0.7.0 → 上游 v0.6.63 → v0.6.62 |
| `src/app.rs`（4 处） | 上游新增 `LAYOUT_SCALE` 全局缩放 | 逐处按坐标系合并，见第五节 |
| `src/ui_tests.rs` | 上游把硬编码坐标改为响应矩形定位 | 采用上游版本 |
| `src/wechat.rs` | Listener 已迁往 `messenger_flash.rs` | 删除旧块，补回 `describe`/`describe_foreground`/`trace` |
| `src/main.rs` | 开关提示 + 钉钉状态清理 | 两边保留 |
| `app.rc` | 版本号 0.6.63 vs 0.7.0 | 取 0.7.0 |
| `src/model.rs` | `sysmon_metrics` 字段 | 取自己版本 |

**结果**：7 个提交全部重放成功，`cargo check` 通过。

---

## 五、阶段 3：LAYOUT_SCALE 与上游测试的冲突（最重要的一课）

### 事实

`LAYOUT_SCALE = 1.4` 是**自己** v0.7.0 引入的全局放大，**上游从来没有**。
上游所有布局测试按 419×140 未缩放基准写。

### 踩坑链条

1. 看到上游某个分支写了 `236.0 * LAYOUT_SCALE`，就给**所有**分支都乘 → `expanded_height` 返回 1.4 倍 → 6 个布局测试全挂
2. 又怀疑 `content_origin` 坐标系有问题，把实现改了 → 上游本来是对的，白改
3. 还试过把 `WIN_W`/`WIN_H`/`CALENDAR_H` 改回不缩放 → 反过来破坏了自己的设计

### 最终正确解法

| 常量/函数 | 正确写法 |
|---|---|
| `WIN_W` / `WIN_H` / `CALENDAR_H` | **乘** `LAYOUT_SCALE`（这是自己设计的基准，要放大） |
| `expanded_height` 返回值 | **逻辑坐标**，不乘；缩放由 `Card` 统一做 |
| 电量圆环半径 `6.56` | 绘制在 `Card` 之外，**不缩放** |
| 测试里的阈值 `width > 50` | **保持上游原值**，不要乘（含义是过滤噪声） |
| 测试里的尺寸断言 `46`/`18`/`radius 6.56` | 按 `×1.4` 换算 |
| `hit.height()` 断言 | `176*1.4 + 2.0 = 248.4`（`expand2` 的 2.0 不参与缩放） |

### 关键方法论：先跑基线

```powershell
git checkout upstream/main                  # 纯上游
cargo test <失败用例> -- --exact --test-threads=1
```

本次结论：**纯上游 6 个用例全过** → 失败是合并引入，必须修，不是上游的锅。
同时验证**自己的备份分支 `e124787`** 也全过 → 说明缩放方案本身自洽，问题在 rebase 时 `ui_tests.rs` 混入了上游版本。

### 结果

781/870 → 修完 **787 passed / 0 failed / 83 ignored**。

---

## 六、阶段 4：编码与 BOM 污染

| 问题 | 现象 | 修法 |
|---|---|---|
| 编辑误删 BOM | `model.rs`、`wechat.rs` diff 出现整行噪声 | 用 `UTF8Encoding($true)` 写回 |
| `.ps1` 无 BOM | PowerShell 5.1 按 GBK 解析中文 → 引号错配 → 假语法错 | 存成 **UTF-8 with BOM** |
| here-string | `Write-Host @"..."@` 被解析成重定向 | 改多条 `Write-Host` |
| 提交信息带 BOM | `git commit -F` 读文件带进 BOM | 改用 `--amend -m` |

---

## 七、阶段 5：第二次合并上游 PR #94 + #95（2026-11-11）

这次上游推了两个 PR：

- `19caaf0` 合并 PR #94：腾讯企业邮箱能力列表尾空格解析
- `f046d6c` 合并 PR #95：HTTP 代理下 HTTPS 请求格式（含 `vendor/ureq` 2.12.1 导入、Grok Bot 额度 403 修复）

### 动作

```powershell
git fetch upstream
git checkout mine
git merge upstream/main        # → 96b4044（merge commit）
```

```powershell
git checkout main
git merge --ff-only upstream/main    # 39413b3 → f046d6c
# 59 files changed, 13754 insertions(+), 2 deletions(-)
```

### 与第一次的差异

| | 第一次（v0.6.63） | 第二次（PR #94/#95） |
|---|---|---|
| `mine` 合上游的方式 | **rebase**（线性） | **merge**（产生 `96b4044`） |
| 冲突 | 18 个文件 | 极少 |
| `main` | `reset --hard` | `merge --ff-only` |

> `mine` 当前**不是纯线性**，含一个 merge commit `96b4044`。
> 若日后要线性化，必须走第九节的流程，切勿直接 `rebase --onto`。

---

## 八、阶段 6：网络阻断与推送（本阶段最大不确定因素）

### 现象

| 测试 | 结果 |
|---|---|
| DNS 解析 `github.com` | ✅ |
| TCP `github.com:443` | ❌ 超时 |
| `curl github.com` | ❌ `http=000` |
| `curl api.github.com` | ✅ 有响应（403 限流） |
| 本机代理端口 7890/7897/10809/1080 | ❌ 无监听 |

**结论**：域级阻断，不是 git 配置问题。

### 应对：探测通过才推，不盲目重试

```powershell
for ($i = 1; $i -le 120; $i++) {
  $code = curl.exe -s -o NUL -w "%{http_code}" --max-time 8 https://github.com/
  if ($code -eq "200" -or $code -eq "301" -or $code -eq "302") {
    git push origin main
    if ($LASTEXITCODE -eq 0) { break }
  }
  Start-Sleep -Seconds 15
}
```

**实际过程**：前 14 轮 `http=000`，第 15 轮（13:57:31）恢复 → 推送成功。

```
b4f58a9..f046d6c  main -> main
```

### 注意

推送脚本里 `2>&1` 会触发 PowerShell 的 `NativeCommandError`（因为 git 把 `To https://...` 写到 stderr），
**这是假报错**，判断成功要看 `$LASTEXITCODE` 和 git 的 `a..b main -> main` 那一行，不要看 PS 的异常。

推完再跑 `git ls-remote` 可能因网络再次抖动而超时，**不代表推送失败**，看本地 `origin/main` ref 即可。

---

## 九、⚠️ 重大失误复盘：rebase --onto 弄丢了一个提交

### 发生了什么

为把 `96b4044` 的 merge 历史改写为线性，执行了：

```powershell
git rebase --onto f046d6c 2feb8a9 427095d
```

**结果：`mine` 变成 `f046d6c` 本身，`427095d` 这个提交一度丢失。**

### 根因

`--onto` 的三个参数语义没对齐：

```
git rebase --onto <newbase> <upstream> <branch>
                            ↑ 这里的 "upstream" 指"哪些提交视为已有"
```

传 `2feb8a9` 时，`2feb8a9` 之后到 `427095d` 的提交都被判定为"已存在"，于是无提交可重放，
分支直接落在 `f046d6c` 上。

### 恢复

```powershell
git reset --hard origin/mine     # 回到 427095d
```

`reflog` 里也完整记录，`git reflog` 可找回。

### 守则（已并入 `fork-sync-notes.md`）

1. 任何改写历史的命令执行前，**先建备份分支指向当前 tip**
   （本次幸有 `backup/mine-before-linear-20261111`）
2. `--onto` 三个参数**逐个核对语义**，不确定就停下来问
3. 改写后立刻验证：`git log --oneline -5`、`git rev-list --count <备份>..<分支>` 是否符合预期
4. **改坏一次就停手**，不要在同一条命令上试第二轮

> 这条失误正是 `fork-sync-notes.md` 第零节"改两轮还不对就立刻停手"规则的现实来源。

---

## 十、日常同步标准流程

### 一条命令

```powershell
.\scripts\sync-upstream.ps1 -Check      # 只检查：守卫结果 + 上游有多少新提交
.\scripts\sync-upstream.ps1            # 完整：fetch → main ff → mine rebase → cargo check
```

脚本已内置 **main 纯镜像前置守卫**：`main` 不是上游的祖先/后代关系时直接报错停下。

### 手动等价流程

```powershell
git branch backup/<日期>-before-sync && git tag backup-<日期>
git fetch upstream
git checkout main && git merge --ff-only upstream/main
git checkout mine && git rebase main          # 或 merge upstream/main
cargo check --locked --all-targets --features record
cargo test --locked --all-targets --features record -- --test-threads=1   # 后台跑
git push origin mine --force-with-lease        # 仅 rebase 后需要
```

### 验证顺序（不可颠倒）

```
cargo check  →  cargo test --lib  →  完整 cargo test
```

失败时**先跑上游基线**，分清"上游自带"还是"合并引入"。

---

## 十一、五条铁律

1. **`main` 永不提交**，只允许 `merge --ff-only`。它是基准，不是工作区。
2. **不用 GitHub 网页的 "Sync fork" 按钮**，它会直接改 `origin/main`，绕过本地历史。
3. **rebase 后推 `mine` 必须 `--force-with-lease`**，绝不用 `--force`。
4. **冲突拿不准就停下来问**，不猜、不试；同一处改两轮还不对立刻停手。
5. **改写历史前先建备份分支**，`reflog` 兜底但备份更可靠。

### 附带两条

6. **提交后不自动 push**，等明确确认再推。
7. **网络阻断时不改配置硬闯**，用"探测通过才推"的循环，等待恢复。

---

## 十二、遗留项

| 项 | 说明 |
|---|---|
| `mine` 含 merge commit `96b4044` | 目前保留，不强制线性化；若要线性化按第九节流程 |
| `LAYOUT_SCALE` 与上游测试摩擦 | 上游每次动布局都要调断言，属已知成本 |
| 备份分支与 tag 未推送远端 | 仅本地，可接受 |
| `vendor/ureq` 由上游引入 | 不动，依赖变更走 `Cargo.lock` |
