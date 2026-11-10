# syntax=docker/dockerfile:1.4

# go.mod replaces the operator module with ../runtime-operator, so builds pass
# it as a named context: docker build --build-context runtime-operator=runtime-operator runtime-gateway
FROM golang:1.27.2@sha256:820153096fda4de6ac1b7194d12e416b7dd8f4e9a33d17e0345b111b945162d8 AS builder
WORKDIR /workspace/runtime-gateway

# dependencies cache
COPY --from=runtime-operator go.mod go.sum ../runtime-operator/
COPY go.mod go.sum ./
RUN go mod download

# source code
COPY --from=runtime-operator . ../runtime-operator/
COPY . .
RUN CGO_ENABLED=0 go build -o /go/bin/runtime-gateway ./

FROM cgr.dev/chainguard/static:latest@sha256:fc42af33e643f20d08baf03cddcc1c02ba57052dfb02edb0d79f3fc47bcea8d9 AS release
COPY --from=builder /go/bin/runtime-gateway /usr/local/bin/runtime-gateway
ENTRYPOINT ["/usr/local/bin/runtime-gateway"]
