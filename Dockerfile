ARG NIX_VERSION=2.17.0
ARG DEBIAN_VERSION=12.1

FROM --platform=$BUILDPLATFORM nixos/nix:${NIX_VERSION} as build
ARG TARGETPLATFORM
COPY . /src
WORKDIR /src
RUN case ${TARGETPLATFORM} in \
    "darwin/amd64")  TARGET="x86_64-apple-darwin" ;; \
    "darwin/arm64")  TARGET="aarch64-apple-darwin" ;; \
    "linux/amd64")   TARGET="x86_64-unknown-linux-musl" ;; \
    "linux/arm64")   TARGET="aarch64-unknown-linux-musl" ;; \
    "windows/amd64") TARGET="x86_64-pc-windows-gnu" ;; \
    *) \
    echo "ERROR: TARGETPLATFORM '${TARGETPLATFORM}' not supported." \
    exit 1 \
    ;; \
    esac &&\
    nix --accept-flake-config --extra-experimental-features 'nix-command flakes' build -L ".#wasmcloud-${TARGET}" &&\
    install -Dp ./result/bin/wasmcloud /out/wasmcloud

FROM debian:${DEBIAN_VERSION}-slim as result

RUN apt update &&\
    apt install -y ca-certificates

COPY --from=build /out/wasmcloud /bin/wasmcloud

CMD ["wasmcloud"]
