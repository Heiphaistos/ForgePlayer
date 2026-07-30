@echo off
setlocal EnableDelayedExpansion
title ForgePlayer — Setup
echo.
echo  ██████  ███    ███ ███    ██ ██ ██████  ██       █████  ██    ██ ███████ ██████
echo ██    ██ ████  ████ ████   ██ ██ ██   ██ ██      ██   ██  ██  ██  ██      ██   ██
echo ██    ██ ██ ████ ██ ██ ██  ██ ██ ██████  ██      ███████   ████   █████   ██████
echo ██    ██ ██  ██  ██ ██  ██ ██ ██ ██      ██      ██   ██    ██    ██      ██   ██
echo  ██████  ██      ██ ██   ████ ██ ██      ███████ ██   ██    ██    ███████ ██   ██
echo.
echo  Setup des dependances — ForgePlayer
echo =============================================================

REM ── Vérification Rust ──────────────────────────────────────────
echo [1/6] Verification de Rust...
where rustc >nul 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo  [!] Rust non installe. Telechargement de rustup...
    powershell -Command "Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile '%TEMP%\rustup-init.exe'"
    "%TEMP%\rustup-init.exe" -y --default-toolchain stable --target x86_64-pc-windows-msvc i686-pc-windows-msvc
    set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
) else (
    echo  [OK] Rust detecte:
    rustc --version
)

REM ── Vérification Go ────────────────────────────────────────────
echo [2/6] Verification de Go...
where go >nul 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo  [!] Go non installe. Telechargez Go sur https://go.dev/dl/
    echo      et relancez ce script.
    pause
    exit /b 1
) else (
    echo  [OK] Go detecte:
    go version
)

REM ── Téléchargement FFmpeg ──────────────────────────────────────
echo [3/6] Verification de FFmpeg...
if exist "C:\ffmpeg\lib\avcodec.lib" (
    echo  [OK] FFmpeg deja present dans C:\ffmpeg
) else (
    echo  [~~] Telechargement de FFmpeg 7.x (shared libs, x64)...
    set FFMPEG_URL=https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl-shared.zip
    set FFMPEG_ZIP=%TEMP%\ffmpeg.zip

    powershell -Command "Invoke-WebRequest -Uri '%FFMPEG_URL%' -OutFile '%FFMPEG_ZIP%'"
    if %ERRORLEVEL% NEQ 0 (
        echo  [ERREUR] Echec du telechargement FFmpeg.
        echo  Telechargez manuellement depuis https://github.com/BtbN/FFmpeg-Builds/releases
        echo  et extrayez dans C:\ffmpeg
        pause
        exit /b 1
    )

    echo  [~~] Extraction dans C:\ffmpeg...
    powershell -Command "Expand-Archive -Path '%FFMPEG_ZIP%' -DestinationPath 'C:\ffmpeg_tmp' -Force"
    powershell -Command "Move-Item 'C:\ffmpeg_tmp\ffmpeg-master-latest-win64-gpl-shared' 'C:\ffmpeg' -Force" 2>nul || (
        for /d %%D in (C:\ffmpeg_tmp\ffmpeg-*) do (
            powershell -Command "Move-Item '%%D' 'C:\ffmpeg' -Force"
        )
    )
    rmdir /s /q C:\ffmpeg_tmp 2>nul
    del /f /q "%FFMPEG_ZIP%" 2>nul
    echo  [OK] FFmpeg installe dans C:\ffmpeg
)

REM ── Variables d'environnement ──────────────────────────────────
echo [4/6] Configuration des variables d'environnement...
setx FFMPEG_DIR "C:\ffmpeg" /M >nul 2>&1 || setx FFMPEG_DIR "C:\ffmpeg"
set "FFMPEG_DIR=C:\ffmpeg"

REM Ajout du PATH système
setx PATH "%PATH%;C:\ffmpeg\bin" /M >nul 2>&1
set "PATH=%PATH%;C:\ffmpeg\bin"
echo  [OK] FFMPEG_DIR=C:\ffmpeg

REM ── Cibles Rust ────────────────────────────────────────────────
echo [5/6] Ajout cibles Rust (x64 + x32)...
rustup target add x86_64-pc-windows-msvc
rustup target add i686-pc-windows-msvc
echo  [OK] Cibles ajoutees

REM ── Dépendances Go ─────────────────────────────────────────────
echo [6/6] Telechargement dependances Go...
go mod tidy
echo  [OK] Modules Go prets

echo.
echo =============================================================
echo  [✓] Setup termine ! Lancez build.bat pour compiler.
echo =============================================================
echo.
pause
