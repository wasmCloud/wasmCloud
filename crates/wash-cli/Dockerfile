ARG NIX_VERSION=2.18.1
ARG DEBIAN_VERSION=12.2

FROM --platform=$BUILDPLATFORM nixos/nix:${NIX_VERSION} as build
ARG TARGETPLATFORM
COPY ../.. /src
WORKDIR /src
RUN case ${TARGETPLATFORM} in \
    "darwin/amd64")  TARGET="x86_64-apple-darwin" ;; \
    "darwin/arm64")  TARGET="aarch64-apple-darwin" ;; \
    "linux/amd64")   TARGET="x86_64-unknown-linux-musl" ;; \
    "linux/arm64")   TARGET="aarch64-unknown-linux-musl" ;; \
    *) \
    echo "ERROR: TARGETPLATFORM '${TARGETPLATFORM}' not supported." \
    exit 1 \
    ;; \
    esac &&\
    nix --accept-flake-config --extra-experimental-features 'nix-command flakes' build -L ".#wasmcloud-${TARGET}"
RUN install -Dp ./result/bin/wash /out/wash

FROM debian:${DEBIAN_VERSION}-slim as result

RUN apt update &&\
    apt install -y ca-certificates

ARG USERNAME=wash
ARG USER_UID=1000
ARG USER_GID=$USER_UID

RUN addgroup --gid $USER_GID $USERNAME \
    && adduser --disabled-login -u $USER_UID --ingroup $USERNAME $USERNAME

USER $USERNAME

COPY --from=build --chown=$USERNAME --chmod=755 /out/wash /bin/wash

CMD ["wash"]
