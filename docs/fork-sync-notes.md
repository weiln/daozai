# Fork 同步上游：踩坑记录与操作守则

本文件记录 2026-11 首次把上游 `Renly1994/xiaotang-daozai` 的 v0.6.63（`39413b3`）
合入本 fork 时的实际问题、根因和教训。下次同步前**先读这份**，能省下大部分时间。

---

## 一、分支模型（不要改）

```
upstream/main   原作者 Renly1994/xiaotang-daozai，只 fetch，push 地址设为 DISABLE
origin         你自己的 fork（weiln/daozai）
main            纯上游镜像。只允许 fast-forward，永不提交
mine            你的功能分支。所有自定义提交都在这里，永远 rebase 到 upstream/main
backup/*        动手前的安全快照，出问题随时回退
```

**铁律**

1. `main` 上永远不留自己的提交。一旦留了，上游再更新就无法 `--ff-only`。
2. 同步只用 `git fetch upstream && git rebase upstream/main`，
   **不要用 GitHub 网页的 "Sync fork" 按钮**，它会绕过本地历史直接改 `origin/main`。
3. 动手前先建备份：`git branch backup/<日期>-before-sync` + `git tag backup-<日期>`。
4. 日常同步走 `scripts/sync-upstream.ps1`：
   - `-Check` 只看有没有新提交
   - 默认 fetch → main 快进 → mine rebase → cargo check
   - 冲突时脚本停在第 2 步之前并打印处理步骤，不会静默失败

---

## 二、环境问题

### 1. 网络：github.com 主站会被阻断，api.github.com 正常

现象：`git fetch` 超时，但 `curl https://api.github.com` 200。

- 不是 git 配置问题，是本机网络对 `github.com` 这个域的限制
- 排障顺序：DNS 解析 → TCP 443 → `curl -sS -o NUL -w "%{http_code}"` → 本机代理端口是否有监听
- 网络恢复后会间歇性抖动，`fetch` 返回 0 但没写进 refs：
  **看到"成功"要用 `git rev-parse upstream/main` / `git ls-remote` 再确认一次**
- CI / 脚本里遇到失败，直接重试，不要急着改配置

### 2. cargo 不在 PATH

在 `%USERPROFILE%\.cargo\bin`。脚本里已自动补 PATH，手动执行时注意。

### 3. 完整测试要 7–8 分钟

`cargo test --all-targets --features record` 包含真实 Win32 窗口操作。
单次工具调用容易被 120 秒上限中断，**用后台任务跑，然后读日志**，不要反复轮询。

---

## 三、编码与文件格式（最坑的一块）

### BOM 必须保住

仓库里几个文件是 **UTF-8 with BOM**（`src/model.rs`、`src/wechat.rs`、`scripts/sync-upstream.ps1`）。
各种编辑工具（`edit`、PowerShell 重写）会**悄悄把 BOM 去掉**，导致：

- `git diff` 出现整行"噪声"改动
- Windows PowerShell 5.1 按本地编码解析中文 → 引号配对错乱 →
  `表达式或语句中包含意外的标记` / `字符串缺少终止符` 这类**假报错**

修法：

```powershell
$path = "scripts\sync-upstream.ps1"
$text = [System.IO.File]::ReadAllText($path, [System.Text.Encoding]::UTF8)
[System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding($true)))
```

**每次改完 .ps1 文件，都要跑一遍解析校验**：

```powershell
$errors = $null
[System.Management.Automation.Language.Parser]::ParseFile((Resolve-Path $path), [ref]$null, [ref]$errors) | Out-Null
if ($errors) { $errors | ForEach-Object { $_.Message } } else { "PARSE OK" }
```

### here-string 里不要放大段带缩进的帮助文本

`Write-Host @"..."@` 在 PS 5.1 里容易被解析成重定向。用多条 `Write-Host` 逐行输出更稳。

---

## 四、冲突解决：这次真正踩过的坑

### 4.1 不要"看着像"就复制

本次最严重的错误：`expanded_height` 里看到上游某一分支写了 `236.0 * LAYOUT_SCALE`，
就顺手给**所有**分支都乘了 `LAYOUT_SCALE`。实际上返回值是**逻辑坐标**，
缩放由调用方 `Card` 统一做，多乘一次让面板高度变成 1.4 倍，6 个布局测试全挂。

**守则**：冲突时先搞清楚这个坐标/尺寸属于**哪套坐标系**（逻辑 vs 屏幕），
注释里写了"实际绘制再 × LAYOUT_SCALE"的地方就不要再乘。

### 4.2 先分清"上游自带失败"还是"我合并引入"

第一件事永远是**在纯上游代码上跑同样的测试**：

```powershell
git checkout upstream/main   # 或 git worktree add 一个干净目录
cargo test <那几个用例名> -- --exact --test-threads=1
```

本次结论：纯上游 6 个用例全过 → 失败是合并引入的，必须修，不是上游的锅。

同样要验证**自己的原始分支**（备份分支）是否通过，
这样能判断"是我的实现和新上游测试基准冲突"还是"我改坏了"。

### 4.3 上游改了基准，测试要跟着改

本次真正的根因之一：

| 项 | 你的原版 | 上游 |
|---|---|---|
| `mail_list_bounds` 的 y 起点 | 36.0 | **54.0**（新增了筛选栏） |
| 测试点击点 | `H::local(300, 50)` | 未改 |

y=50 在原版落在 36–122 内能命中，在上游落在 54 之外就点不到了。
**这是上游的正当改动，改测试坐标，不要改实现去迁就测试。**

### 4.4 阈值类断言要按缩放换算

`lyric_caps_lock` 的"只允许 1 条玻璃描边"断言：
上游阈值 `width > 50 && height >= 20` 是按未缩放写的。
你把阈值改成 `>50*1.4` 反而把宽度 70 的消息角标也算了进去 → 误报 2 条。
**结论：阈值含义是"过滤噪声"，不是"尺寸断言"，保持上游原值即可。**

### 4.5 有些东西压根不参与缩放

电量圆环半径 `6.56` 绘制在 `Card` 之外，**不乘 LAYOUT_SCALE**。
而按钮尺寸 `46×18×1.4` 会缩放。断言重叠时要分别对待。

### 4.6 不要把删除范围搞大

钉钉重构把 `State`/`subclass`/`Listener` 从 `wechat.rs` 整体搬到 `messenger_flash.rs`，
rebase 时删旧块**连带删掉了同区块里的 `trace`/`describe`/`describe_foreground`**，
编译时才发现 5 个错误。

**守则**：删代码块前先用 `grep` 列出这块里所有的 `fn` 定义，
删完立刻 `cargo check`，不要攒到最后。

---

## 五、提交整理

- 修编译/测试的补丁**优先并入对应提交**让历史干净：
  `git commit --fixup <目标> && git rebase -i --autosquash`
- 但 **autosquash 遇到整行冲突时很容易改坏文件**（本次发生过一次，最后 `git rebase --abort` 退回）。
  稳妥做法：把这些适配改动**单独合成一个**说明清楚的提交
  （如"合入上游 vX：适配缩放布局、清理重复监听代码"），可读性一样好，风险小得多。
- 提交信息开头不要带 BOM（用 `git commit -F` 从文件读时注意）。

---

## 六、每次同步的检查清单

```
[ ] git branch backup/<日期>-before-sync && git tag backup-<日期>
[ ] .\scripts\sync-upstream.ps1 -Check      # 看上游有多少新提交
[ ] .\scripts\sync-upstream.ps1            # 冲突就手工解，解完 --continue
[ ] cargo check --locked --all-targets --features record
[ ] cargo test --locked --all-targets --features record -- --test-threads=1   （后台跑）
[ ] 若有失败：先在 upstream/main 上复现，确认是否上游自带
[ ] git status 确认没有 BOM / 空白噪声
[ ] git push origin mine --force-with-lease
```

### 降低以后冲突率的做法

1. 一个功能一个 commit，越小越好 drop / cherry-pick
2. 能拆成新文件（`src/messenger_flash.rs` 那样）就别改 `app.rs` / `pages.rs` 里的高频段
3. 改上游文件时，把你的定制逻辑收敛到独立函数或独立常量块，别散落在上游代码中间
4. 上游每发版就同步一次，别攒 10 个版本
