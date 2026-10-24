<#
.SYNOPSIS
    Windows 构建脚本：自动选择 MSVC 或 GNU 工具链并打包 NSIS 安装包。

.DESCRIPTION
    这台机器上的 MSVC Build Tools 可能没装（只剩 Visual Studio Installer），
    而 MinGW 自带的 GNU ld 2.30 太老，链接不动新版 Rust 的产物。
    本脚本会自动探测，有 MSVC 就走 MSVC，没有就走 GNU + workaround，
    保证 `npm run build:win` 在两种环境下都能出安装包。

.PARAMETER Msvc
    强制使用 MSVC 工具链。

.PARAMETER Bundles
    要打的安装包类型，默认 nsis。

.EXAMPLE
    npm run build:win
#>
param(
  [switch]$Msvc,
  [string]$Bundles = 'nsis'
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $projectRoot

try {
  # ---------------------------------------------------------------- #
  # 1. 探测 MSVC 工具链
  # ---------------------------------------------------------------- #
  $msvcInstall = $null
  $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  if (Test-Path $vswhere) {
    $found = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
    if ($found) {
      $toolsRoot = Join-Path $found 'VC\Tools\MSVC'
      if (Test-Path $toolsRoot) {
        $version = Get-ChildItem $toolsRoot -Directory | Sort-Object Name -Descending | Select-Object -First 1
        if ($version) {
          $link = Join-Path $version.FullName 'bin\Hostx64\x64\link.exe'
          if (Test-Path $link) { $msvcInstall = $found }
        }
      }
    }
  }

  $useMsvc = $msvcInstall -ne $null
  if ($Msvc -and -not $useMsvc) {
    throw '指定了 -Msvc，但没有找到 MSVC 工具链（需要 Visual Studio Build Tools 的 C++ 组件）'
  }

  # 每次构建前先清掉上一轮的 workaround，避免两种工具链串味
  $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = $null
  $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = $null
  $env:RC = $null

  if ($useMsvc) {
    $target = 'x86_64-pc-windows-msvc'
    Write-Host '[build] MSVC 工具链：' -ForegroundColor Green
    Write-Host "        $msvcInstall"
  }
  else {
    $target = 'x86_64-pc-windows-gnu'
    Write-Host '[build] 未找到 MSVC，改用 GNU 工具链 + workaround' -ForegroundColor Yellow

    # --- rust-lld：替代过老的 GNU ld ---
    $lld = Get-ChildItem "$env:USERPROFILE\.rustup\toolchains" -Recurse -Filter 'rust-lld.exe' -ErrorAction SilentlyContinue |
      Where-Object { $_.FullName -like '*x86_64-pc-windows-msvc*' } |
      Sort-Object FullName -Descending |
      Select-Object -First 1
    if (-not $lld) {
      throw '找不到 rust-lld（rustup toolchain 里应该自带）'
    }
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = $lld.FullName

    # --- MinGW 的库路径和启动对象 ---
    $gcc = (Get-Command gcc -ErrorAction SilentlyContinue).Source
    if (-not $gcc) {
      throw '找不到 gcc（MinGW），请把 MinGW 的 bin 加进 PATH，或改用 -Msvc'
    }
    $mingwRoot = Split-Path -Parent (Split-Path -Parent $gcc)

    $gccLibRoot = Join-Path $mingwRoot 'lib\gcc\x86_64-w64-mingw32'
    $gccVersion = $null
    if (Test-Path $gccLibRoot) {
      $gccVersion = Get-ChildItem $gccLibRoot -Directory | Sort-Object Name -Descending | Select-Object -First 1
    }

    $searchPaths = @(
      (Join-Path $mingwRoot 'x86_64-w64-mingw32\lib'),
      (Join-Path $mingwRoot 'lib')
    )
    if ($gccVersion) {
      $searchPaths += $gccVersion.FullName
    }

    $rustflags = @('-Clinker-flavor=ld.lld')
    foreach ($path in $searchPaths) {
      if (Test-Path $path) { $rustflags += "-Lnative=$path" }
    }
    # 直接用 lld 时 rustc 不再经 gcc 传启动对象，缺了会报 mainCRTStartup 未定义
    $rustflags += '-Clink-arg=crt2.o'
    $rustflags += '-Clink-arg=-lgcc_eh'
    $rustflags += '-Clink-arg=--start-group'

    $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = $rustflags -join ' '

    # --- 资源编译器：windres 代替 rc.exe ---
    $env:RC = Join-Path $PSScriptRoot 'windres-shim.cmd'

    Write-Host "        linker : $($lld.FullName)"
    Write-Host "        mingw  : $mingwRoot"
    Write-Host "        RC     : $($env:RC)"

    # GNU 分支下 cdylib 的启动对象和 exe 不一样（要 DllMainCRTStartup），
    # 手动塞的 crt2.o 只管 exe，链接 cdylib 会报 undefined symbol。
    # 桌面端用不到 cdylib，打包期间临时摘掉，打完恢复。
    $cargoToml = Join-Path $projectRoot 'src-tauri\Cargo.toml'
    $cargoTomlBackup = [System.IO.File]::ReadAllText($cargoToml)
    $patched = $cargoTomlBackup -replace 'crate-type = \["staticlib", "cdylib", "rlib"\]', 'crate-type = ["staticlib", "rlib"]'
    if ($patched -ne $cargoTomlBackup) {
      Write-Host '        crate-type 临时去掉 cdylib（GNU 降级路径的取舍，桌面端不需要）' -ForegroundColor DarkGray
      [System.IO.File]::WriteAllText($cargoToml, $patched)
    }
  }

  Write-Host "[build] target = $target, bundles = $Bundles"
  Write-Host ''

  # ---------------------------------------------------------------- #
  # 2. 构建
  # ---------------------------------------------------------------- #
  try {
    & npm.cmd run tauri:build -- --target $target --bundles $Bundles
    if ($LASTEXITCODE -ne 0) {
      throw "构建失败，退出码 $LASTEXITCODE"
    }
  }
  finally {
    # GNU 降级分支临时改过 Cargo.toml，这里必须还原。
    if ($null -ne $cargoTomlBackup) {
      [System.IO.File]::WriteAllText($cargoToml, $cargoTomlBackup)
    }
  }

  # ---------------------------------------------------------------- #
  # 3. 免安装压缩包
  # ---------------------------------------------------------------- #
  # 单个 exe 就是自包含的（前端资源已经嵌进 exe），解压即用，不用装。
  $releaseRoot = Join-Path $projectRoot "src-tauri\target\$target\release"
  $mainExe = Join-Path $releaseRoot 'naiwa-pet.exe'

  if (Test-Path $mainExe) {
    $portableDir = Join-Path $projectRoot 'dist-portable'
    if (Test-Path $portableDir) {
      [System.IO.Directory]::Delete($portableDir, $true)
    }
    [System.IO.Directory]::CreateDirectory($portableDir) | Out-Null
    Copy-Item $mainExe (Join-Path $portableDir '奶蛙.exe')

    $readme = @"
奶蛙 · 免安装版
================

解压后直接双击「奶蛙.exe」就能用，不需要安装。

第一次运行
----------
- Windows 可能弹「Windows 已保护你的电脑」，点「更多信息」→「仍要运行」。
  这是因为程序没有买代码签名证书，不是病毒。
- 桌面上没看到奶蛙的话，看任务栏右下角，可能收在 ^ 展开的隐藏图标区。

第一次要填两样东西（聊天窗口顶部有张「欢迎使用奶蛙」的卡片）
----------------------------------------------------------
1. API Key —— 必填，没有它没法跟模型说话。
   默认已经内置了一个共享 Key，直接点「保存」也能用；
   想用自己的，去设置里换（也可以填 DeepSeek 之类别的服务的 Key 和地址）。
2. 麦当劳 MCP Token —— 选填。不填就完全没有麦当劳功能。
   想要的话点卡片上的「在这里直接登录（不用切浏览器）」，
   页面里点右上角「登录」→ 手机扫二维码登录 → 回首页点「控制台」→「激活」申请 Token
   → 复制回来粘到输入框 → 保存。填完立刻生效，不用重启。
   复制按钮要是没反应，点 Token 输入框 Ctrl+A 全选、Ctrl+C 手动复制一样能用。
   也可以点 open.mcd.cn/mcp 用系统浏览器打开，手机上更方便。

怎么用
------
- 右键奶蛙 →「和奶蛙聊一聊」，问一句「查一下贵州茅台现在的行情」试试
- 聊天窗口在奶蛙左边；点行情卡片会另开一个图表窗口，有分时 / 日线 / 周线 / 月线
  四个页签，K 线带 MA5/10/20/60 均线，鼠标移上去有十字光标
- 也能问麦当劳：「附近有麦当劳吗」「巨无霸多少大卡」「我手上有什么券」「我有多少积分」
- 让它下单的话，它会先把餐品、优惠和总价说清楚，再问你「要我下单吗？」，
  你点头了它才真的下单。付款会给你一个二维码，手机扫一下就行
- 右键奶蛙 →「设置…」可以换 API Key、模型、地址，也可以随时清掉麦当劳 Token
- 奶蛙会自己冒泡：想聊的时候主动说话；你自选股涨跌超过 2% 也会提醒你
  （自选股让它帮你加：「把茅台加到自选」）

数据存在哪
----------
设置和自选股存在 %APPDATA%\naiwa-pet\ 下，卸载时删掉这个目录即可。

常见问题
--------
- 复制 Token 时页面提示「复制失败」：点 Token 输入框 Ctrl+A 全选、Ctrl+C 手动复制。
- 双击没反应：看任务栏右下角 ^ 展开的隐藏图标区。
"@
    $readmePath = Join-Path $portableDir '使用说明.txt'
    [System.IO.File]::WriteAllText(
      $readmePath,
      $readme,
      (New-Object System.Text.UTF8Encoding($true))
    )

    $zipPath = Join-Path $projectRoot 'dist-portable.zip'
    if (Test-Path $zipPath) {
      [System.IO.File]::Delete($zipPath)
    }
    Compress-Archive -Path (Join-Path $portableDir '*') -DestinationPath $zipPath -CompressionLevel Optimal
    "        免安装包: $zipPath  ({0} MB)" -f [math]::Round((Get-Item $zipPath).Length / 1MB, 2)
  }

  # ---------------------------------------------------------------- #
  # 4. 报告产物
  # ---------------------------------------------------------------- #
  $bundleRoot = Join-Path $projectRoot "src-tauri\target\$target\release\bundle"
  Write-Host ''
  Write-Host '[build] 产物：' -ForegroundColor Green
  if (Test-Path $bundleRoot) {
    Get-ChildItem $bundleRoot -Recurse -File -Include '*.exe', '*.msi' |
      ForEach-Object {
        "        {0}  ({1} MB)" -f $_.FullName.Replace($projectRoot, '.'), [math]::Round($_.Length / 1MB, 2)
      }
  }
  $mainExe = Join-Path $projectRoot "src-tauri\target\$target\release\naiwa-pet.exe"
  if (Test-Path $mainExe) {
    "        {0}  ({1} MB)" -f $mainExe.Replace($projectRoot, '.'), [math]::Round((Get-Item $mainExe).Length / 1MB, 2)
  }
}
finally {
  Pop-Location
}
