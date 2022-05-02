FROM rust:alpine as builder

WORKDIR /build
RUN apk add --no-cache clang clang-dev libressl-dev ca-certificates musl-dev llvm-dev clang-libs curl gcompat
RUN curl -LO https://github.com/protocolbuffers/protobuf/releases/download/v3.15.8/protoc-3.15.8-linux-x86_64.zip && \
    mkdir -p $HOME/.local/bin && \
    unzip protoc-3.15.8-linux-x86_64.zip -d $HOME/.local
ENV PATH="${HOME}/.local/bin:${PATH}"

COPY Cargo.toml .
COPY Cargo.lock .
COPY ./src ./src

RUN adduser \    
    --disabled-password \    
    --gecos "" \    
    --home "/home/user" \    
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid "10001" \    
    "user"

ENV RUSTFLAGS=-Ctarget-feature=-crt-static
RUN cargo build --release

FROM alpine as release-alpine
WORKDIR /home/user
RUN apk add --no-cache bash curl libgcc libressl-dev ca-certificates musl-dev

COPY --from=builder /etc/passwd /etc/passwd
COPY --from=builder /etc/group /etc/group

COPY --from=builder /build/target/release/wash /usr/local/bin/

USER user:user

FROM scratch

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /etc/passwd /etc/passwd
COPY --from=builder /etc/group /etc/group

COPY --from=builder /usr/lib/libssl.so.* /usr/lib/
COPY --from=builder /usr/lib/libcrypto.so.* /usr/lib/
COPY --from=builder /usr/lib/libgcc_s.so.* /usr/lib/
COPY --from=builder /lib/ld-musl-x86_64.so.* /lib/

COPY --from=builder /build/target/release/wash /usr/local/bin/

USER user:user
ENTRYPOINT ["/usr/local/bin/wash"]
CMD ["-V"]
