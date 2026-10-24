<#
.SYNOPSIS
    给奶蛙的 exe / 安装包做代码签名。

.DESCRIPTION
    未签名的程序会被 Windows SmartScreen 拦下来（用户点"更多信息 → 仍要运行"，
    下载来源越不正越容易被杀软直接拦），这是分发环节最后一块硬骨头。

    本脚本从证书存储里按 Subject 关键字找到代码签名证书，用 Windows SDK 的
    signtool 签名，并带 RFC3161 时间戳（时间戳保证签名在证书过期后依然有效）。

    证书需要自己申请：OV 代码签名证书（几千元/年），或用 Azure Artifact
    Signing（约 $10/月）/ SignPath.io（开源项目可申请免费）。

.PARAMETER Subject
    证书 Subject 里要匹配的关键字，例如 "Naiwa"。

.PARAMETER Files
    要签名的文件。默认签打包产物：主程序 + NSIS 安装包。

.EXAMPLE
    .\scripts\sign-windows.ps1 -Subject "Naiwa"
#>
param(
  [Parameter(Mandatory = $true)][string]$Subject,
  [string[]]$Files = @()
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot

if ($Files.Count -eq 0) {
  $releaseRoot = Join-Path $projectRoot 'src-tauri\target\x86_64-pc-windows-msvc\release'
  $candidates = @(
    (Join-Path $releaseRoot 'naiwa-pet.exe'),
    (Join-Path $releaseRoot 'bundle\nsis\奶蛙_0.1.0_x64-setup.exe')
  )
  $Files = $candidates | Where-Object { Test-Path $_ }
}

if ($Files.Count -eq 0) {
  throw '没有找到要签名的文件，请先跑 npm run build:win'
}

# ---------------------------------------------------------------- #
# 1. 找证书
# ---------------------------------------------------------------- #
Write-Host '[sign] 在证书存储里查找代码签名证书...' -ForegroundColor Cyan
$certs = Get-ChildItem Cert:\CurrentUser\My, Cert:\LocalMachine\My -ErrorAction SilentlyContinue |
  Where-Object {
    $_.HasPrivateKey -and
    $_.Subject -like "*$Subject*" -and
    ($_.EnhancedKeyUsageList.ObjectId -contains '1.3.6.1.5.5.7.3.3')
  }

if (-not $certs) {
  Write-Host @"
[sign] 没找到带私钥的代码签名证书（Subject 含 "$Subject"）。

  代码签名证书需要自己申请，三条路：
    1. 商业 OV 证书（DigiCert / Sectigo / GlobalSign 等），几千元/年，个人和企业都能买
    2. Azure Artifact Signing，微软官方，约 \$10/月，按签名次数计费
    3. SignPath.io，开源项目可申请免费签名

  申请下来后，把 .pfx 导入「证书 - 当前用户 - 个人」即可。
"@ -ForegroundColor Yellow
  exit 1
}

$cert = $certs | Sort-Object NotAfter | Select-Object -Last 1
Write-Host "[sign] 使用证书: $($cert.Subject)"
Write-Host "[sign] 指纹    : $($cert.Thumbprint)"
Write-Host "[sign] 有效期  : $($cert.NotBefore) ~ $($cert.NotAfter)"

if ($cert.NotAfter -lt (Get-Date).AddDays(30)) {
  Write-Host '[sign] 警告：证书 30 天内就要过期了' -ForegroundColor Yellow
}

# ---------------------------------------------------------------- #
# 2. 找 signtool
# ---------------------------------------------------------------- #
$signtool = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin' -Recurse -Filter 'signtool.exe' -ErrorAction SilentlyContinue |
  Where-Object { $_.FullName -like '*\x64\*' } |
  Sort-Object FullName -Descending |
  Select-Object -First 1

if (-not $signtool) {
  throw '找不到 signtool.exe，需要装 Windows SDK'
}
Write-Host "[sign] signtool: $($signtool.FullName)"

# ---------------------------------------------------------------- #
# 3. 签名
# ---------------------------------------------------------------- #
$timestamp = 'http://timestamp.digicert.com'

foreach ($file in $Files) {
  Write-Host ''
  Write-Host "[sign] 签名 $($file)" -ForegroundColor Cyan
  & $signtool.FullName sign /fd SHA256 /tr $timestamp /td SHA256 /sha1 $cert.Thumbprint $file
  if ($LASTEXITCODE -ne 0) {
    throw "签名失败：$file"
  }
}

# ---------------------------------------------------------------- #
# 4. 验证
# ---------------------------------------------------------------- #
Write-Host ''
Write-Host '[sign] 验证签名状态：' -ForegroundColor Green
foreach ($file in $Files) {
  $status = (Get-AuthenticodeSignature $file).Status
  "{0,-16} {1}" -f $status, (Split-Path -Leaf $file)
}

Write-Host ''
Write-Host '[sign] 全部完成。签名后 SmartScreen 不再拦截，但首次仍可能提示"未知发布者"——' -ForegroundColor Green
Write-Host '       这是信誉积累问题，多几个人装过、微软信誉库更新后就不再提示了。'
