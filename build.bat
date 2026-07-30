@echo off
setlocal EnableDelayedExpansion
title ForgePlayer — Build
echo.
echo  [BUILD] ForgePlayer — Compilation Rust + Go
echo =============================================================

REM ── Vérifie FFMPEG_DIR ─────────────────────────────────────────
if "%FFMPEG_DIR%"=="" (
    if exist "C:\ffmpeg\lib\avcodec.lib" (
        set "FFMPEG_DIR=C:\ffmpeg"
    ) else (
        echo  [ERREUR] FFMPEG_DIR non defini. Lancez setup.bat d'abord.
        pause & exit /b 1
    )
)
echo  [~] FFMPEG_DIR=%FFMPEG_DIR%

REM ── Parse les arguments ────────────────────────────────────────
set TARGET=release
set ARCH=x64
set SKIP_GO=0

:parse_args
if "%~1"=="debug"   (set "TARGET=debug"   & shift & goto parse_args)
if "%~1"=="release" (set "TARGET=release" & shift & goto parse_args)
if "%~1"=="x32"     (set "ARCH=x32"       & shift & goto parse_args)
if "%~1"=="x64"     (set "ARCH=x64"       & shift & goto parse_args)
if "%~1"=="skip-go" (set "SKIP_GO=1"      & shift & goto parse_args)
if "%~1"=="" goto do_build
shift & goto parse_args

:do_build

REM ── Cible Rust ────────────────────────────────────────────────
if "%ARCH%"=="x32" (
    set RUST_TARGET=i686-pc-windows-msvc
    set FFMPEG_LIB_DIR=%FFMPEG_DIR%\lib
) else (
    set RUST_TARGET=x86_64-pc-windows-msvc
    set FFMPEG_LIB_DIR=%FFMPEG_DIR%\lib
)
echo  [~] Architecture : %ARCH% [%RUST_TARGET%]
echo  [~] Mode         : %TARGET%

REM ── Compile Rust ──────────────────────────────────────────────
echo.
echo  [1/3] Compilation Rust (%TARGET%)...

if "%TARGET%"=="release" (
    set CARGO_FLAGS=--release
) else (
    set CARGO_FLAGS=
)

set FFMPEG_DIR=%FFMPEG_DIR%
cargo build %CARGO_FLAGS% --target %RUST_TARGET% -p forgeplayer 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo  [ERREUR] Compilation Rust echouee.
    pause & exit /b 1
)
echo  [OK] Rust compile

REM ── Compile Go ────────────────────────────────────────────────
if "%SKIP_GO%"=="0" (
    echo.
    echo  [2/3] Compilation Go ^(services^)...

    if "%ARCH%"=="x32" (
        set GOARCH=386
    ) else (
        set GOARCH=amd64
    )
    set GOOS=windows
    set CGO_ENABLED=0

    go build -o dist\subtitle-service.exe  .\cmd\subtitle-service\
    if %ERRORLEVEL% NEQ 0 ( echo  [WARN] subtitle-service build echoue )

    go build -o dist\media-indexer.exe     .\cmd\media-indexer\
    if %ERRORLEVEL% NEQ 0 ( echo  [WARN] media-indexer build echoue )

    echo  [OK] Services Go compiles
) else (
    echo  [2/3] Services Go — IGNORE ^(skip-go^)
)

REM ── Assemble le dossier dist ───────────────────────────────────
echo.
echo  [3/3] Assemblage dist\...

if "%TARGET%"=="release" (
    set RUST_BIN=target\%RUST_TARGET%\release\forgeplayer.exe
) else (
    set RUST_BIN=target\%RUST_TARGET%\debug\forgeplayer.exe
)

if not exist dist mkdir dist

REM Copie l'exécutable Rust principal
copy /Y "%RUST_BIN%" dist\ForgePlayer.exe >nul
if %ERRORLEVEL% NEQ 0 (
    echo  [ERREUR] Executable Rust introuvable : %RUST_BIN%
    pause & exit /b 1
)

REM Copie les DLLs FFmpeg (nécessaires au runtime)
if exist "%FFMPEG_DIR%\bin\avcodec-*.dll" (
    copy /Y "%FFMPEG_DIR%\bin\av*.dll"    dist\ >nul
    copy /Y "%FFMPEG_DIR%\bin\sw*.dll"    dist\ >nul
    copy /Y "%FFMPEG_DIR%\bin\postproc*.dll" dist\ >nul 2>nul
    echo  [OK] DLLs FFmpeg copiees dans dist\
) else (
    echo  [WARN] DLLs FFmpeg non trouvees dans %FFMPEG_DIR%\bin
)

REM Copie les assets (shaders)
xcopy /E /I /Y assets dist\assets >nul

echo.
echo =============================================================
echo  [✓] Build termine !
echo     Executable  : dist\ForgePlayer.exe
if "%SKIP_GO%"=="0" (
    echo     Services    : dist\subtitle-service.exe
    echo                   dist\media-indexer.exe
)
echo     Lancez dist\ForgePlayer.exe pour demarrer.
echo =============================================================
echo.
pause
