# syntax=docker/dockerfile:1
FROM rclone/rclone:1.75.2 AS rclone

FROM ubuntu:24.04 AS builder
ARG TARGETARCH
ARG QT_VERSION=6.12.0

RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential cmake ninja-build python3-venv ca-certificates \
      libgl1-mesa-dev libxkbcommon-dev libfontconfig1-dev libdbus-1-dev \
    && rm -rf /var/lib/apt/lists/* \
    && python3 -m venv /opt/aqt \
    && /opt/aqt/bin/pip install --no-cache-dir aqtinstall==3.3.0

RUN case "$TARGETARCH" in \
      amd64) qt_host=linux; qt_arch=linux_gcc_64; qt_dir=gcc_64 ;; \
      arm64) qt_host=linux_arm64; qt_arch=linux_gcc_arm64; qt_dir=gcc_arm64 ;; \
      *) echo "Unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && /opt/aqt/bin/aqt install-qt "$qt_host" desktop "$QT_VERSION" "$qt_arch" \
      --outputdir /opt/Qt --archives icu qtbase qtsvg qttranslations \
    && ln -s "/opt/Qt/$QT_VERSION/$qt_dir" /opt/qt-sdk

COPY .build/source/ /source/
WORKDIR /source
RUN python3 scripts/validate_qt_version.py --version "$QT_VERSION" \
    && cmake -S . -B /build -G Ninja -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_PREFIX_PATH=/opt/qt-sdk -DCMAKE_INSTALL_PREFIX=/usr/local \
    && cmake --build /build --parallel 2 \
    && QT_QPA_PLATFORM=offscreen ctest --test-dir /build --output-on-failure --parallel 2 \
    && cmake --install /build \
    && mkdir -p /opt/qt \
    && cp -a /opt/qt-sdk/lib /opt/qt-sdk/plugins /opt/qt-sdk/translations /opt/qt/ \
    && printf '[Paths]\nPrefix=/opt/qt\n' > /usr/local/bin/qt.conf

FROM jlesage/baseimage-gui:ubuntu-24.04-v4.14.0
RUN add-pkg ca-certificates fonts-dejavu-core xterm libgl1 libegl1 libopengl0 \
      libdbus-1-3 libfontconfig1 libfreetype6 libglib2.0-0t64 libx11-xcb1 \
      libxkbcommon-x11-0 libxcb-cursor0 libxcb-icccm4 libxcb-image0 \
      libxcb-keysyms1 libxcb-render-util0 libxcb-randr0 libxcb-shape0 \
      libxcb-xfixes0 libxcb-xinerama0 libxcb-sync1 libxcb-xkb1 libsm6 libice6 \
      libxrender1 libxi6 libssl3t64 libgssapi-krb5-2 \
    && set-cont-env APP_NAME 'Rclone Browser NG' \
    && mkdir -p /media
COPY --from=builder /usr/local/bin/rclone-browser /usr/local/bin/qt.conf /usr/local/bin/
COPY --from=builder /usr/local/share/ /usr/local/share/
COPY --from=builder /opt/qt/ /opt/qt/
COPY --from=rclone /usr/local/bin/rclone /usr/local/bin/rclone
COPY --chmod=755 docker/startapp.sh /startapp.sh
ENV LD_LIBRARY_PATH=/opt/qt/lib QT_PLUGIN_PATH=/opt/qt/plugins
WORKDIR /media
