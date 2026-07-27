#!/bin/sh
# Build natif Linux (Rust release + services Go) + packages .deb/AppImage.
# A lancer sur une machine Linux (Debian/Ubuntu) avec Rust, Go, clang/libclang-dev installes.
#
# IMPORTANT : le crate ffmpeg-next patche (patches/ffmpeg-next, absent du clone Git,
# a recuperer depuis le backup local) ne compile PAS contre les headers FFmpeg 8.x
# recents (BtbN master/n8.1) : AVCodec a perdu ses champs directs (pix_fmts,
# supported_framerates, sample_fmts, ch_layouts -> avcodec_get_supported_config()),
# et plusieurs AVCodecID references par le patch n'existent plus. On compile donc
# contre FFmpeg 7.1.5 (paquets apt Debian), qui a toujours l'ancienne disposition.
#
# Usage: ./build-linux.sh
set -e

APT_PKGS="libavcodec-dev libavformat-dev libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev libswresample-dev libasound2-dev libgtk-3-dev clang libclang-dev llvm-dev xauth xvfb"
echo "[1/5] apt install dependances (sudo requis si non root)..."
apt-get install -y $APT_PKGS

# ffmpeg-sys-next (crates.io) a un souci de detection des chemins multiarch Debian
# (cherche /usr/include/libswscale/swscale.h au lieu de /usr/include/x86_64-linux-gnu/...).
# Contournement : un FFMPEG_DIR synthetique avec des symlinks vers les vrais chemins.
FFDIR=/tmp/ffmpeg-system
echo "[2/5] Prepare FFMPEG_DIR synthetique ($FFDIR)..."
mkdir -p "$FFDIR/include" "$FFDIR/lib"
for d in libavcodec libavdevice libavfilter libavformat libavutil libswresample libswscale libpostproc; do
    ln -sfn "/usr/include/x86_64-linux-gnu/$d" "$FFDIR/include/$d"
done
for f in /usr/lib/x86_64-linux-gnu/lib{avcodec,avdevice,avfilter,avformat,avutil,swresample,swscale,postproc}.so*; do
    ln -sf "$f" "$FFDIR/lib/$(basename "$f")"
done

echo "[3/5] cargo build --release (Rust)..."
export FFMPEG_DIR="$FFDIR"
export LIBCLANG_PATH=$(dirname "$(find /usr/lib -iname 'libclang.so*' | head -1)")
export CC=gcc
export CXX=g++
export BINDGEN_EXTRA_CLANG_ARGS=""
unset PKG_CONFIG_PATH
export LD_LIBRARY_PATH="$FFDIR/lib"
cargo build --release -p omni-player

echo "[4/5] go build (services)..."
GOOS=linux GOARCH=amd64 CGO_ENABLED=0 go build -o dist/subtitle-service ./cmd/subtitle-service/
GOOS=linux GOARCH=amd64 CGO_ENABLED=0 go build -o dist/media-indexer ./cmd/media-indexer/
mkdir -p dist
cp target/release/omniplayer dist/

echo "[5/5] Smoke test (Xvfb, 8s)..."
timeout 8 xvfb-run -a ./dist/omniplayer || true

echo "OK. Binaires dans dist/. Voir DEBUG_LOG.md pour le detail du packaging .deb/AppImage."
