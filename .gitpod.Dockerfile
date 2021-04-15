FROM gitpod/workspace-full

# Gitpod will not rebuild dev image unless *some* change is made to this Dockerfile.
# To force a rebuild, simply increase this counter:
ENV TRIGGER_REBUILD 7

USER gitpod

RUN sudo apt-get update && \
    sudo apt-get install -y \
    libssl-dev \
    libxcb-composite0-dev \
    pkg-config \    
    rust-lldb \
    redis-server \
    && sudo rm -rf /var/lib/apt/lists/*

RUN wget -c https://dl.google.com/go/go1.14.2.linux-amd64.tar.gz -O - | sudo tar -xz -C /usr/local
RUN export PATH=$PATH:/usr/local/go/bin


ENV GO111MODULE=on
RUN sudo /usr/local/go/bin/go get github.com/nats-io/nats-server/v2

RUN nats-server &

ENV RUST_LLDB=/usr/bin/lldb-11