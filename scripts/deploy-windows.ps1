# PowerShell 部署脚本 - 将编译产物收集到 bin 目录并打包
# 用法: .\scripts\deploy-windows.ps1 [release|debug]

param(
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"

Write-Host "=== 任务图编辑器 部署脚本 ===" -ForegroundColor Cyan
Write-Host "编译配置: $Profile" -ForegroundColor Green

# 项目根目录
$RootDir = Split-Path -Parent $PSScriptRoot

# 从 Cargo.toml 提取版本号
$CargoToml = Get-Content (Join-Path $RootDir "Cargo.toml") -Raw
if ($CargoToml -match 'version\s*=\s*"([^"]+)"') {
    $Version = $Matches[1]
} else {
    Write-Host "错误: 无法从 Cargo.toml 提取版本号" -ForegroundColor Red
    exit 1
}
Write-Host "版本号: v$Version" -ForegroundColor Green

# 目录定义
$BinDir = Join-Path $RootDir "bin"
$TargetDir = Join-Path $RootDir "target\$Profile"

# Step 1: 清理旧的 bin 目录
Write-Host ""
Write-Host "[1/3] 清理旧的 bin 目录..." -ForegroundColor Yellow
if (Test-Path $BinDir) {
    Remove-Item -Recurse -Force $BinDir
    Write-Host "  已删除旧目录: $BinDir" -ForegroundColor Gray
}

# Step 2: 复制可执行文件
Write-Host ""
Write-Host "[2/3] 复制可执行文件..." -ForegroundColor Yellow
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
$ExeName = "task-graph-editor.exe"
$ExePath = Join-Path $TargetDir $ExeName
if (Test-Path $ExePath) {
    Copy-Item $ExePath -Destination $BinDir
    $ExeSize = [math]::Round((Get-Item $ExePath).Length / 1MB, 1)
    Write-Host "  已复制: $ExeName (${ExeSize}MB)" -ForegroundColor Green
} else {
    Write-Host "  错误: 找不到可执行文件 $ExePath" -ForegroundColor Red
    Write-Host "  请先运行: cargo build --release" -ForegroundColor Yellow
    exit 1
}

# Step 3: 创建版本压缩包
Write-Host ""
Write-Host "[3/3] 创建版本压缩包..." -ForegroundColor Yellow

# 写入 VERSION 文件
"v$Version" | Out-File -FilePath (Join-Path $BinDir "VERSION") -Encoding UTF8 -NoNewline

$ZipName = "task-graph-editor-v$Version.zip"
$ZipPath = Join-Path $BinDir $ZipName

# 删除旧的 zip 文件
Get-ChildItem -Path $BinDir -Filter "task-graph-editor-v*.zip" -ErrorAction SilentlyContinue | Remove-Item -Force

# 创建新的 zip 文件
Push-Location $BinDir
try {
    $ItemsToCompress = Get-ChildItem -Path $BinDir -Exclude "*.zip"
    Compress-Archive -Path $ItemsToCompress.FullName -DestinationPath $ZipPath -Force
    Write-Host "  已创建: bin/$ZipName" -ForegroundColor Green
} finally {
    Pop-Location
}

# 部署完成提示
Write-Host ""
Write-Host "=== 部署完成 ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "目录结构:" -ForegroundColor Green
Write-Host "bin/"
Write-Host "├── $ExeName"
Write-Host "├── VERSION"
Write-Host "└── $ZipName"
Write-Host ""
Write-Host "运行方式:" -ForegroundColor Green
Write-Host "  cd bin"
Write-Host '  .\task-graph-editor.exe'
Write-Host ""
Write-Host "分发方式:" -ForegroundColor Green
Write-Host "  将 bin/$ZipName 发送给用户"
Write-Host ""
