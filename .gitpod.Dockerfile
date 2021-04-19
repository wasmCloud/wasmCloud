FROM gitpod/workspace-full
LABEL maintainer="team@wasmcloud.com"

USER gitpod

RUN sudo apt-get update && \
    sudo apt-get install -y \
    libssl-dev \
    libxcb-composite0-dev \
    pkg-config \    
    rust-lldb \
    redis-server \
    && sudo rm -rf /var/lib/apt/lists/*


RUN curl -L https://github.com/nats-io/nats-server/releases/download/v2.2.1/nats-server-v2.2.1-linux-amd64.zip -o nats-server.zip
RUN unzip nats-server.zip -d nats-server
RUN sudo cp nats-server/nats-server-v2.2.1-linux-amd64/nats-server /usr/bin

RUN /usr/bin/nats-server &

ENV RUST_LLDB=/usr/bin/lldb-11

RUN rustup component add clippy
RUN rustup target add wasm32-unknown-unknown
RUN cargo install wasmcloud wash-cli
