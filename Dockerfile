FROM rust:alpine as builder

WORKDIR /build
RUN apk add --no-cache clang clang-dev libressl-dev ca-certificates musl-dev llvm-dev clang-libs

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
COPY --from=builder /lib/libssl.so.48 /lib/libssl.so.48
COPY --from=builder /lib/libcrypto.so.46 /lib/libcrypto.so.46
COPY --from=builder /usr/lib/libgcc_s.so.1 /usr/lib/libgcc_s.so.1
COPY --from=builder /lib/ld-musl-x86_64.so.1 /lib/ld-musl-x86_64.so.1

COPY --from=builder /build/target/release/wash /usr/local/bin/

USER user:user
ENTRYPOINT ["/usr/local/bin/wash"]
CMD ["-V"]
