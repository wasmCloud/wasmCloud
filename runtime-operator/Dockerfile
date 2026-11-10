# syntax=docker/dockerfile:1.4

FROM golang:1.27.2@sha256:820153096fda4de6ac1b7194d12e416b7dd8f4e9a33d17e0345b111b945162d8 AS builder
WORKDIR /workspace/runtime-operator

# dependencies cache
COPY go.mod go.sum ./
RUN go mod download

# source code
COPY . .
RUN CGO_ENABLED=0 go build -o /go/bin/runtime-operator ./cmd

FROM cgr.dev/chainguard/static:latest@sha256:fc42af33e643f20d08baf03cddcc1c02ba57052dfb02edb0d79f3fc47bcea8d9 AS release
COPY --from=builder /go/bin/runtime-operator /usr/local/bin/runtime-operator
ENTRYPOINT ["/usr/local/bin/runtime-operator"]
