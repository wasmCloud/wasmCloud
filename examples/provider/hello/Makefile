# examples/provider/hello/Makefile

WELD = ../../../target/debug/weld
# if this package is included in the top-level Cargo.toml, then target is in the top-level folder
TARGET_BASE=../../../target
# otherwise, it's local
#TARGET_BASE=./target
CARGO=cargo
RUST_DEPS = Cargo.toml $(wildcard src/*) Makefile

CAPID = wasmcloud:example:hello
NAME = "Hello Provider (Rust)"
VENDOR = "wasmCloud"
REVISION = 1

TARGET_LINUX = x86_64-unknown-linux-gnu
TARGET_MAC = x86_64-apple-darwin

PROJECT = $(shell $(WELD) toml-json Cargo.toml | jq -r '.package.name') 
VERSION = $(shell $(WELD) toml-json Cargo.toml | jq -r '.package.version')

RELEASE_TARGET_LINUX = $(TARGET_BASE)/$(TARGET_LINUX)/release/$(PROJECT)
DEBUG_TARGET_LINUX = $(TARGET_BASE)/$(TARGET_LINUX)/debug/$(PROJECT)
RELEASE_TARGET_MAC = $(TARGET_BASE)/$(TARGET_MAC)/release/$(PROJECT)
DEBUG_TARGET_MAC = $(TARGET_BASE)/$(TARGET_MAC)/debug/$(PROJECT)


DIST_BIN = build/hello-linux
DIST_PAR = build/hello.par.gz
DIST_EXS = build/exs.out


all:: $(DIST_BIN) $(DIST_PAR) $(DIST_EXS)

clean:
	$(CARGO) clean
	rm -rf build $(DIST_BIN) $(DIST_PAR) $(DIST_EXS)

#build-linux: $(DEBUG_TARGET_LINUX)
#build-mac: $(DEBUG_TARGET_MAC)
#release-linux: $(RELEASE_TARGET_LINUX)
#release-mac: $(RELEASE_TARGET_MAC)

.ONESHELL:
$(DIST_EXS): $(DIST_BIN) $(DIST_PAR)
	@mkdir -p $(dir $@)
	@cat <<- EOF > $@
	%{
		name: $(NAME),
		path: "$(abspath $(DIST_BIN))",
		key: "$(shell wash par inspect $(DIST_PAR) -o json | jq -r ".service")",
		link: "default",
		contract: "$(CAPID)",
	},
	EOF

$(DIST_BIN): $(RELEASE_TARGET_LINUX)
	@mkdir -p $(dir $@)
	cp -p --force $< $@

$(DEBUG_TARGET_LINUX): $(WELD) $(RUST_DEPS)
	$(CARGO) build --target $(TARGET_LINUX)

$(DEBUG_TARGET_MAC): $(WELD) $(RUST_DEPS)
	$(CARGO) build --target $(TARGET_MAC)

$(RELEASE_TARGET_LINUX): $(WELD) $(RUST_DEPS)
	$(CARGO) build --release --target $(TARGET_LINUX)

$(RELEASE_TARGET_MAC): $(WELD) $(RUST_DEPS)
	$(CARGO) build --release --target $(TARGET_MAC)


$(DIST_PAR): $(RELEASE_TARGET_LINUX)
	@mkdir -p $(dir $@)
	wash par create --arch x86_64-linux \
		--binary $(RELEASE_TARGET_LINUX) \
		--capid $(CAPID) \
		--name $(NAME) \
		--vendor $(VENDOR) \
		--version $(VERSION) \
		--revision $(REVISION) \
		--destination $(DIST_PAR) \
		--compress

#	wash par insert $(PAR_NAME) \
#		--arch $(PAR_MAC) \
#		--binary $(RELEASE_TARGET_MAC)
#

inspect claims: $(DIST_PAR)
	wash par inspect $(DIST_PAR)

.PHONY: all build release clean
