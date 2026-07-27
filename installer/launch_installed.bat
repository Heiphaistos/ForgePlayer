@echo off
REM Lance les services Go en arrière-plan puis démarre OmniPlayer.
REM Variante "installée" de launch.bat : les exe sont à plat dans ce même dossier
REM (pas de sous-dossier dist\, contrairement au repo de dev).
setlocal

set DIST=%~dp0

REM Clés API (à renseigner ou définir dans les variables d'environnement)
if "%OPENSUBTITLES_API_KEY%"=="" set OPENSUBTITLES_API_KEY=votre_cle_ici
if "%TMDB_API_KEY%"==""          set TMDB_API_KEY=votre_cle_ici

REM ── Service sous-titres + métadonnées ─────────────────────────
echo [launch] Demarrage du service sous-titres (port 18080)...
start /B "" "%DIST%subtitle-service.exe" ^
    --port 18080 ^
    --sub-key  "%OPENSUBTITLES_API_KEY%" ^
    --tmdb-key "%TMDB_API_KEY%" ^
    --lang fr

REM ── Service indexeur médias ───────────────────────────────────
echo [launch] Demarrage de l'indexeur medias (port 18081)...
start /B "" "%DIST%media-indexer.exe" ^
    --port 18081 ^
    --tmdb-key "%TMDB_API_KEY%"

REM Petit délai pour laisser les services démarrer
timeout /t 1 /nobreak >nul

REM ── Lecteur principal ─────────────────────────────────────────
echo [launch] Demarrage OmniPlayer...
"%DIST%OmniPlayer.exe" %*
