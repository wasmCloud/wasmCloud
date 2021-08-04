# capability-provider/httpserver-rs/Makefile

CAPABILITY_ID = wasmcloud:httpserver
NAME = "HttpServer"
VENDOR = "wasmCloud"
PROJECT = httpserver
# first two-parts of the semver version
VERSION = 0.1
# the tag in the registry will be VERSION.REVISION, and must be incremented
# any time the binary changes.
# During development iterations, REVISION should be incremented each build
REVISION = 5

# the debug build is used by host.sh for testing
all: build par

include ../build/makefiles/provider.mk

