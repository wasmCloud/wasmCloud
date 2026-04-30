# syntax=docker/dockerfile:1.4

FROM golang:1.26 AS builder
WORKDIR /workspace/runtime-gateway

# dependencies cache
COPY go.mod go.sum ./
RUN go mod download

# source code
COPY . .
RUN CGO_ENABLED=0 go build -o /go/bin/runtime-gateway ./

FROM cgr.dev/chainguard/static:latest AS release
COPY --from=builder /go/bin/runtime-gateway /usr/local/bin/runtime-gateway
ENTRYPOINT ["/usr/local/bin/runtime-gateway"]
