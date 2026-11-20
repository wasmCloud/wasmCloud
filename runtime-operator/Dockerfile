# syntax=docker/dockerfile:1.4

FROM golang:1.27.4@sha256:205c7636d74e0ffbe3d007162df7d74f859f5a8c8322dfb9f907977686c126cf AS builder
WORKDIR /workspace/runtime-operator

# dependencies cache
COPY go.mod go.sum ./
RUN go mod download

# source code
COPY . .
RUN CGO_ENABLED=0 go build -o /go/bin/runtime-operator ./cmd

FROM cgr.dev/chainguard/static:latest@sha256:8658e9eaeb2294fa0e49b0667f7d66b4e21fa7690b8d2a4c30cd1d013ceb9eb7 AS release
COPY --from=builder /go/bin/runtime-operator /usr/local/bin/runtime-operator
ENTRYPOINT ["/usr/local/bin/runtime-operator"]
