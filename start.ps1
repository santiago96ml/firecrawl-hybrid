param(
    [switch]$Rebuild
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path

function Resolve-Binary([string]$portablePath, [string]$repoBuildPath, [string]$name) {
    if (-not $Rebuild -and (Test-Path $portablePath)) { return $portablePath }
    if (-not $Rebuild -and (Test-Path $repoBuildPath)) { return $repoBuildPath }
    return $null
}

$goBin = Resolve-Binary (Join-Path $root "go-engine.exe") (Join-Path $root "go-engine\go-engine.exe") "go-engine"
if (-not $goBin) {
    Write-Host "Compilando go-engine.exe..."
    Push-Location (Join-Path $root "go-engine")
    try {
        go build -ldflags="-s -w" -o go-engine.exe .
        if ($LASTEXITCODE -ne 0) { throw "Fallo la compilacion de go-engine" }
    } finally { Pop-Location }
    $goBin = Join-Path $root "go-engine\go-engine.exe"
}

$rsBin = Resolve-Binary (Join-Path $root "firecrawl-rs.exe") (Join-Path $root "rust-core\target\release\firecrawl-rs.exe") "firecrawl-rs"
if (-not $rsBin) {
    Write-Host "Compilando firecrawl-rs.exe (cargo build --release)..."
    Push-Location (Join-Path $root "rust-core")
    try {
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "Fallo la compilacion de rust-core" }
    } finally { Pop-Location }
    $rsBin = Join-Path $root "rust-core\target\release\firecrawl-rs.exe"
}

Write-Host "Iniciando Go Engine (:8081) -> $goBin"
$pGo = Start-Process -FilePath $goBin -WorkingDirectory $root -PassThru

Start-Sleep -Milliseconds 700
Write-Host "Iniciando Rust Core (:3002) -> $rsBin"
$pRs = Start-Process -FilePath $rsBin -WorkingDirectory $root -PassThru

Write-Host ""
Write-Host "Firecrawl Hybrid activo (PIDs Go=$($pGo.Id), Rust=$($pRs.Id)):"
Write-Host "  - Go Engine Health:  http://127.0.0.1:8081/health"
Write-Host "  - Firecrawl API v1/v2: http://127.0.0.1:3002/health"
