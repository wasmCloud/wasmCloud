CONST_FILE := src/lib.rs
define readvar
$(or $(shell sed -nE 's/^.*$(1).*"(.*)";$$/\1/p' $(CONST_FILE)),$(error "Cannot find variable $(1)"))
endef

CAPABILITY_ID := $(call readvar,CAPABILITY_ID)
NAME          := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name')
VERSION       := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[].version')
LIBNAME       := $(shell echo $(NAME) | tr '-' '_')
VENDOR        := wasmCloud
REVISION      ?= 0
DESTINATION   ?= httpserver.par.gz

TARGETS := armv7-unknown-linux-gnueabihf \
	aarch64-unknown-linux-gnu \
	x86_64-apple-darwin \
	x86_64-pc-windows-gnu \
	x86_64-unknown-linux-gnu

# not currently supported
MIPS_TARGETS := mips-unknown-linux-gnu \
	mipsel-unknown-linux-gnu \
	mips64-unknown-linux-gnuabi64 \
	mips64el-unknown-linux-gnuabi64

$(TARGETS):
	cross build --target $@ --release

par: $(TARGETS)
	wash par create \
		--arch arm-linux \
		--binary target/armv7-unknown-linux-gnueabihf/release/lib$(LIBNAME).so \
		--capid $(CAPABILITY_ID) \
		--name $(NAME) \
		--vendor $(VENDOR) \
		--version $(VERSION) \
		--revision $(REVISION) \
		--destination $(DESTINATION) \
		--compress
	wash par insert \
		--arch aarch64-linux \
		--binary target/aarch64-unknown-linux-gnu/release/lib$(LIBNAME).so \
		$(DESTINATION)
	wash par insert \
		--arch x86_64-macos \
		--binary target/x86_64-apple-darwin/release/lib$(LIBNAME).dylib \
		$(DESTINATION)
	wash par insert \
		--arch x86_64-windows \
		--binary target/x86_64-pc-windows-gnu/release/$(LIBNAME).dll \
		$(DESTINATION)
	wash par insert \
		--arch x86_64-linux \
		--binary target/x86_64-unknown-linux-gnu/release/lib$(LIBNAME).so \
		$(DESTINATION)

$(MIPS_TARGETS):
	cross build --target $@ --release

par-mips: $(MIPS_TARGETS) par 
	wash par insert \
		--arch mips-linux \
		--binary target/mips-unknown-linux-gnu/release/lib$(LIBNAME).so \
		$(DESTINATION)
	wash par insert \
		--arch mipsel-linux \
		--binary target/mipsel-unknown-linux-gnu/release/lib$(LIBNAME).so \
		$(DESTINATION)
	wash par insert \
		--arch mips64-linux \
		--binary target/mips64-unknown-linux-gnuabi64/release/lib$(LIBNAME).so \
		$(DESTINATION)
	wash par insert \
		--arch mips64el-linux \
		--binary target/mips64el-unknown-linux-gnuabi64/release/lib$(LIBNAME).so \
		$(DESTINATION)
