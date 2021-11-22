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
    --home "/nonexistent" \    
    --shell "/sbin/nologin" \    
    --no-create-home \    
    --uid "10001" \    
    "user"

ENV RUSTFLAGS=-Ctarget-feature=-crt-static
RUN cargo build --release

FROM scratch

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /etc/passwd /etc/passwd
COPY --from=builder /etc/group /etc/group

COPY --from=builder /lib/ld-musl-x86_64.so.1 /lib/ld-musl-x86_64.so.1
COPY --from=builder /usr/lib/libssl.so.48 /usr/lib/libssl.so.48
COPY --from=builder /usr/lib/libcrypto.so.46 /usr/lib/libcrypto.so.46
COPY --from=builder /usr/lib/libgcc_s.so.1 /usr/lib/libgcc_s.so.1
COPY --from=builder /lib/ld-musl-x86_64.so.1 /lib/ld-musl-x86_64.so.1

COPY --from=builder /build/target/release/wash /usr/local/bin/

USER user:user
ENTRYPOINT ["/usr/local/bin/wash"]
CMD ["-V"]
