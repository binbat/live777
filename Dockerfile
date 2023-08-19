FROM rust:1.71.1-slim-bookworm

RUN apt update -y && apt install -y --no-install-recommends libglib2.0-dev libssl-dev \
    libgstreamer1.0-dev gstreamer1.0-tools  \
    libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly \
    libpango1.0-dev libgstreamer-plugins-bad1.0-dev gstreamer1.0-nice

RUN cargo install cargo-c

WORKDIR /src

ADD https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs/-/archive/gstreamer-1.22.5/gst-plugins-rs-gstreamer-1.22.5.tar.gz gst-plugins-rs-gstreamer.tar.gz

RUN tar -xf gst-plugins-rs-gstreamer.tar.gz --strip-components 1

#RUN set -eux; \
#    dpkgArch="$(dpkg --print-architecture)"; \
#    case "${dpkgArch##*-}" in \
#        amd64) archLibPath='x86_64-linux-gnu' ;; \
#        arm64) archLibPath='aarch64-linux-gnu' ;; \
#        *) echo >&2 "unsupported architecture: ${dpkgArch}"; exit 1 ;; \
#    esac; \
#    cargo cinstall -p gst-plugin-webrtchttp --prefix=/usr --libdir=/usr/lib/${archLibPath}
RUN cargo cinstall -p gst-plugin-webrtchttp --prefix=/usr --libdir=/usr/lib/$(gcc -dumpmachine)

# rtpav1pay / rtpav1depay: RTP (de)payloader for the AV1 video codec.
RUN cargo cinstall -p gst-plugin-rtp --prefix=/usr --libdir=/usr/lib/$(gcc -dumpmachine)

#FROM rust:alpine
#RUN apk add --no-cache openssl-dev musl-dev cargo-c gstreamer gstreamer-dev libnice-gstreamer gstreamer-tools gst-plugins-good gst-plugins-base gst-plugins-base-dev gst-plugins-bad gst-plugins-bad-dev
#WORKDIR /src
#RUN cargo cinstall -p gst-plugin-webrtchttp --prefix=/usr

