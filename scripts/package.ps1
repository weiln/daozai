param(
    [switch]$SkipBuild,
    [string]$Executable,
    [string]$NsisCompiler,
    [string]$OutputDirectory,
    # 编译并行度：默认用满逻辑核心，低内存机器传 1 可换取更小的内存峰值。
    [ValidateRange(1, 256)][int]$Jobs = 0
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
Push-Location $projectRoot
$stepTimer = [Diagnostics.Stopwatch]::StartNew()
$timings = [ordered]@{}
function Complete-Step([string]$name) {
    $timings[$name] = $stepTimer.ElapsedMilliseconds
    $stepTimer.Restart()
}
try {
    $version = [regex]::Match((Get-Content Cargo.toml -Raw), '(?m)^version = "([^"]+)"').Groups[1].Value
    # 发布版不嵌入构建机的用户目录和项目路径：依赖源码路径换成中性前缀，PDB 只记文件名。
    # CARGO_ENCODED_RUSTFLAGS 用 0x1f 分隔，避免路径里的空格被拆开；它会覆盖 .cargo/config.toml 的 rustflags，所以带上 crt-static。
    $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $env:CARGO_ENCODED_RUSTFLAGS = @('-C', 'target-feature=+crt-static', "--remap-path-prefix=$cargoHome=cargo", "--remap-path-prefix=$projectRoot=daozai", '-C', 'link-arg=/PDBALTPATH:%_PDB%') -join [char]0x1f
    if (-not $SkipBuild) {
        # 并行编译依赖图（默认用满逻辑核心）；以前固定单线程，426 个依赖 crate 要跑十几分钟。
        $env:CARGO_BUILD_JOBS = if ($Jobs -gt 0) { [string]$Jobs } else { [string][Environment]::ProcessorCount }
        $cargo = Get-Command cargo -ErrorAction SilentlyContinue
        if (-not $cargo) { $cargo = Get-Item (Join-Path $cargoHome 'bin\cargo.exe') }
        & $cargo.Source +stable-x86_64-pc-windows-msvc build --locked --release --bin daozai --target x86_64-pc-windows-msvc
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    }
    Complete-Step 'build'
    if (-not $Executable) {
        $targetDirectory = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $projectRoot 'target' }
        $Executable = Join-Path $targetDirectory 'x86_64-pc-windows-msvc\release\daozai.exe'
    }
    $publishedExe = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Executable)
    if (-not (Test-Path -LiteralPath $publishedExe)) { throw ('Release executable not found: ' + $publishedExe) }
    $binary = Get-Item -LiteralPath $publishedExe
    & (Join-Path $PSScriptRoot 'check-runtime.ps1') -Executable $publishedExe
    if ($binary.VersionInfo.FileVersion -ne $version -and $binary.VersionInfo.FileVersion -ne ("$version" + '.0')) { throw 'Executable version mismatch' }
    # 快速路径：-SimpleMatch 由 .NET 实现，不再把整个发布版解码成字符串；宽字符（UTF-16）段单独再查一次。
    $wide = [IO.File]::ReadAllText($publishedExe, [Text.Encoding]::Unicode)
    foreach ($private in @($env:USERPROFILE, $projectRoot)) {
        if (-not $private) { continue }
        if (Select-String -LiteralPath $publishedExe -Pattern $private -SimpleMatch -Quiet -Encoding utf8 -ErrorAction Ignore) { throw ("Executable embeds a build-machine path: " + $private) }
        if ($wide.Contains($private)) { throw ("Executable embeds a build-machine path: " + $private) }
    }
    Complete-Step 'checks'
    if (-not $NsisCompiler) {
        $compiler = Get-Command makensis.exe -ErrorAction SilentlyContinue
        if ($compiler) { $NsisCompiler = $compiler.Source }
        elseif ($env:NSIS_HOME) { $NsisCompiler = Join-Path $env:NSIS_HOME 'makensis.exe' }
        else { $NsisCompiler = Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe' }
    }
    if (-not (Test-Path -LiteralPath $NsisCompiler)) { throw 'NSIS compiler not found; install NSIS or pass -NsisCompiler' }
    $packageName = ("Daozai-v" + $version + "-windows-x64")
    if (-not $OutputDirectory) { $OutputDirectory = Join-Path $projectRoot 'dist' } else { $OutputDirectory = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory) }
    $outputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
    $stagingDirectory = Join-Path $outputDirectory $packageName
    if (Test-Path -LiteralPath $stagingDirectory) { throw 'Package directory already exists' }
    New-Item -ItemType Directory -Path $stagingDirectory -Force | Out-Null
    Copy-Item -LiteralPath $publishedExe -Destination (Join-Path $stagingDirectory 'daozai.exe')
    # ZIP 保留给免安装使用和应用内更新；普通用户只需下载安装 EXE。
    @('LICENSE','THIRD_PARTY_NOTICES.txt') | ForEach-Object {
        Copy-Item -LiteralPath (Join-Path $projectRoot $_) -Destination $stagingDirectory -Recurse
    }
    Complete-Step 'stage'
    $setup = Join-Path $outputDirectory ("Daozai-Setup-v" + $version + "-x64.exe")
    # 参数逐个传，避免一行过长被编辑器折行。
    $nsisArgs = @('/V2', '/WX', ('/DAPP_VERSION=' + $version), ('/DPROJECT_DIR=' + $projectRoot), ('/DSOURCE_EXE=' + $publishedExe), ('/DOUTPUT_FILE=' + $setup), (Join-Path $projectRoot 'installer\setup.nsi'))
    & $NsisCompiler @nsisArgs
    if ($LASTEXITCODE -ne 0) { throw 'Installer build failed' }
    Complete-Step 'nsis'
    $archive = Join-Path $outputDirectory ($packageName + '.zip')
    # bsdtar 用 deflate 流式压缩，比 Compress-Archive（逐项 .NET 调用）快一个数量级。
    $tar = Join-Path $env:SystemRoot 'system32\tar.exe'
    if (-not (Test-Path -LiteralPath $tar)) { $tar = 'tar.exe' }
    & $tar -a -c -f $archive --options zip:compression=deflate -C $outputDirectory $packageName
    if ($LASTEXITCODE -ne 0) { throw 'Archive build failed' }
    Complete-Step 'zip'
    $sums = @()
    foreach ($file in @($setup, $archive)) {
        if (-not (Test-Path -LiteralPath $file)) { continue }
        $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
        $sums += ($hash + '  ' + [IO.Path]::GetFileName($file))
    }
    if (-not $sums) { throw 'Nothing to checksum' }
    Set-Content -LiteralPath (Join-Path $outputDirectory 'SHA256SUMS.txt') -Value $sums -Encoding ascii
    Complete-Step 'sha256'
    Write-Output $setup
    Write-Output $archive
    $total = ($timings.Values | Measure-Object -Sum).Sum
    Write-Output ''
    Write-Output 'step timings (seconds):'
    foreach ($item in $timings.GetEnumerator()) {
        Write-Output (($item.Key) + ': ' + [math]::Round($item.Value / 1000, 1))
    }
    Write-Output ('total: ' + [math]::Round($total / 1000, 1))
} finally { Pop-Location }
