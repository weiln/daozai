<#
同步上游（Renly1994/xiaotang-daozai）的新版本到本地的 mine 分支。

用法：
  .\scripts\sync-upstream.ps1            # 拉取并 rebase，失败就停下并提示
  .\scripts\sync-upstream.ps1 -Check     # 只检查上游有没有新提交，不改动任何东西
  .\scripts\sync-upstream.ps1 -NoBuild   # rebase 后不编译验证

分支约定：
  main  = 纯上游镜像，只做 fast-forward，绝不在这上面提交
  mine  = 你的功能分支，7 个自定义提交都在这里，永远 rebase 到 main
#>
param(
    [switch]$Check,
    [switch]$NoBuild
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

function Fail($msg) {
    Write-Host ""
    Write-Host $msg -ForegroundColor Red
    exit 1
}

Write-Host "==> 拉取上游..." -ForegroundColor Cyan
git fetch upstream --prune
if ($LASTEXITCODE -ne 0) { Fail "拉取失败，检查网络或代理（github.com 直连超时是本机已知问题）" }

$behind = [int](git rev-list --count mine..upstream/main)
$ahead  = [int](git rev-list --count upstream/main..mine)

Write-Host "    上游新提交: $behind 个   你的提交: $ahead 个"
if ($behind -eq 0) { Write-Host "==> 已是最新，无需同步" -ForegroundColor Green; exit 0 }
if ($Check) {
    Write-Host ""
    git log --oneline mine..upstream/main
    Write-Host ""
    Write-Host "（-Check 模式，未做任何改动。去掉 -Check 执行同步）" -ForegroundColor Yellow
    exit 0
}

# 记住当前分支，结束后切回
$original = (git rev-parse --abbrev-ref HEAD).Trim()

Write-Host "==> 同步 main 到上游（fast-forward）..." -ForegroundColor Cyan
git checkout main
if ($LASTEXITCODE -ne 0) { Fail "切换 main 失败（有未提交改动？先 commit 或 stash）" }
git merge --ff-only upstream/main
if ($LASTEXITCODE -ne 0) { git checkout $original; Fail "main 无法快进：说明 main 上有本地提交。这正是本方案要避免的，请先把提交挪到 mine 分支" }

Write-Host "==> rebase mine 到最新上游..." -ForegroundColor Cyan
git checkout mine
if ($LASTEXITCODE -ne 0) { Fail "切换 mine 失败" }
git rebase upstream/main
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "REBASE 冲突了。处理方法：" -ForegroundColor Red
    Write-Host '  1. 按提示逐个编辑冲突文件，删掉冲突标记行，保留双方逻辑' -ForegroundColor Gray
    Write-Host '  2. git add 冲突文件' -ForegroundColor Gray
    Write-Host '  3. git rebase --continue' -ForegroundColor Gray
    Write-Host '  放弃可用： git rebase --abort' -ForegroundColor Gray
    exit 2
}

if ($NoBuild) { Write-Host "==> 跳过编译检查" -ForegroundColor Yellow; exit 0 }

Write-Host "==> 编译验证..." -ForegroundColor Cyan
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path (Join-Path $cargoBin 'cargo.exe')) { $env:PATH = "$cargoBin;$env:PATH" }
cargo check --locked --all-targets --features record
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Fail "编译失败。上游的接口可能变了，逐条修好后再 git commit --amend 或用 git rebase -i 修改对应提交"
}

Write-Host ""
Write-Host "==> 同步完成，编译通过。" -ForegroundColor Green
Write-Host "    推送到你的 fork（rebase 过，需要 --force-with-lease）：" -ForegroundColor Gray
Write-Host "    git push origin mine --force-with-lease" -ForegroundColor Gray
