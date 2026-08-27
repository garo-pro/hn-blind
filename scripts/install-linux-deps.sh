#!/usr/bin/env bash
# Install the system libraries hn-blind needs to build on Debian/Ubuntu.
#
# Run this once when setting up a Linux dev box; CI calls it through
# .github/actions/linux-build-deps. The list is architecture-agnostic — the
# release workflow builds aarch64 on an arm runner with exactly these packages.
#
# wxWidgets accounts for most of it (wxdragon builds it from source via CMake);
# libclang-dev is for bindgen, and libspeechd-dev is the speech-dispatcher
# backend Prism talks to on Linux — a static Prism links it into our binary, so
# it is a build-time requirement rather than only a runtime one.
set -euo pipefail

sudo apt-get update
sudo apt-get install -y \
  cmake \
  libclang-dev \
  pkg-config \
  libgtk-3-dev \
  libpng-dev \
  libjpeg-dev \
  libgl1-mesa-dev \
  libglu1-mesa-dev \
  libxkbcommon-dev \
  libwayland-dev \
  libexpat1-dev \
  libtiff-dev \
  libwebkit2gtk-4.1-dev \
  libxtst-dev \
  libsm-dev \
  libice-dev \
  libspeechd-dev
