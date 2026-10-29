# syntax=docker/dockerfile:1.4

FROM golang:1.27.2 AS builder
WORKDIR /workspace/runtime-gateway

# dependencies cache
COPY go.mod go.sum ./
RUN go mod download

# source code
COPY . .
RUN CGO_ENABLED=0 GOAMD64=v1 GOTOOLCHAIN=local go build -mod=readonly -trimpath -buildvcs=false \
    -ldflags='-s -w -buildid=' -o /go/bin/runtime-gateway ./

FROM cgr.dev/chainguard/static:latest AS release
COPY --from=builder /go/bin/runtime-gateway /usr/local/bin/runtime-gateway
ENTRYPOINT ["/usr/local/bin/runtime-gateway"]
