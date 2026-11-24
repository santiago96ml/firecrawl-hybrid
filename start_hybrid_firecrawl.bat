@echo off
title Firecrawl Hybrid (Rust Core + Go Engine)
cd /d "%~dp0"
echo ============================================================================
echo   FIRECRAWL HYBRID ENGINE (Rust Core :3002 + Go Engine :8081)
echo ============================================================================
echo.

if exist "%~dp0go-engine.exe" (
    start "Firecrawl Go Engine (:8081)" /min "%~dp0go-engine.exe"
) else if exist "%~dp0go-engine\go-engine.exe" (
    start "Firecrawl Go Engine (:8081)" /min "%~dp0go-engine\go-engine.exe"
) else (
    echo [INFO] No se encontro go-engine.exe, compilando y ejecutando con PowerShell...
    powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1"
    goto :eof
)

timeout /t 1 /nobreak >nul

if exist "%~dp0firecrawl-rs.exe" (
    start "Firecrawl Rust Core (:3002)" "%~dp0firecrawl-rs.exe"
) else if exist "%~dp0rust-core\target\release\firecrawl-rs.exe" (
    start "Firecrawl Rust Core (:3002)" "%~dp0rust-core\target\release\firecrawl-rs.exe"
) else (
    echo [INFO] No se encontro firecrawl-rs.exe, compilando con cargo build --release...
    powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1"
    goto :eof
)

echo.
echo [OK] Ambos motores iniciados:
echo   - Go Engine Health:    http://127.0.0.1:8081/health
echo   - Firecrawl API (n8n): http://127.0.0.1:3002/health
echo.
pause
