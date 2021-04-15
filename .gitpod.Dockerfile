FROM gitpod/workspace-full

# Gitpod will not rebuild dev image unless *some* change is made to this Dockerfile.
# To force a rebuild, simply increase this counter:
ENV TRIGGER_REBUILD 2

USER gitpod

RUN sudo apt-get update && \
    sudo apt-get install -y \
    libssl-dev \
    libxcb-composite0-dev \
    pkg-config \    
    rust-lldb \
    redis-server \
    && sudo rm -rf /var/lib/apt/lists/*

ENV GO111MODULE=on
RUN SUDO go get github.com/nats-io/nats-server/v2

RUN nats-server &

ENV RUST_LLDB=/usr/bin/lldb-11