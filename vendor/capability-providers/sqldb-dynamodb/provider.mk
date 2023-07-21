# provider.mk
#
# common rules for building capability providers
# Some of these rules depend on GNUMakefile >= 4.0
#
# before including this, local project makefile should define the following
# (to override defaults)
# top_targets      # list of targets that are applicable for this project
#

top_targets     ?= all par par-full test clean 

platform_id = $(shell uname -s)
platform = $$( \
	case $(platform_id) in \
		( Linux ) echo $(platform_id) ;; \
		( Darwin ) echo $(platform_id) ;; \
		( * ) echo Unrecognized Platform;; \
	esac )

machine_id = $(shell uname -m )

# name of compiled binary
bin_name ?= $(PROJECT)
dest_par ?= build/$(bin_name).par.gz
link_name ?= default

# If name is not defined, use project
NAME ?= $(PROJECT)

WASH ?= wash

oci_url_base ?= localhost:5000/v2
oci_url      ?= $(oci_url_base)/$(bin_name):$(VERSION)
ifeq ($(WASH_REG_USER),)
	oci_insecure := --insecure
endif

par_targets ?= \
	x86_64-unknown-linux-gnu \
   	x86_64-apple-darwin \
   	aarch64-unknown-linux-gnu \
   	aarch64-apple-darwin \
	armv7-unknown-linux-gnueabihf \
   	x86_64-pc-windows-gnu

# Lookup table from rust target triple to wasmcloud architecture doubles
# Thanks to https://stackoverflow.com/a/40919906 for the pointer to
# "constructed macro names".
ARCH_LOOKUP_x86_64-unknown-linux-gnu=x86_64-linux
ARCH_LOOKUP_x86_64-apple-darwin=x86_64-macos
ARCH_LOOKUP_armv7-unknown-linux-gnueabihf=arm-linux
ARCH_LOOKUP_aarch64-unknown-linux-gnu=aarch64-linux
ARCH_LOOKUP_aarch64-apple-darwin=aarch64-macos
ARCH_LOOKUP_x86_64-pc-windows-gnu=x86_64-windows

bin_targets = $(foreach target,$(par_targets),target/$(target)/release/$(bin_name))

# pick target0 for starting par based on default rust target
par_target0 ?= $(shell rustup show | grep 'Default host' | sed "s/Default host: //")

# the target of the current platform, as defined by cross
cross_target0=target/$(par_target0)/release/$(bin_name)
# bin_target0=$(cross_target0)
bin_target0=target/release/$(bin_name)

# traverse subdirs
.ONESHELL:
ifneq ($(subdirs),)
$(top_targets)::
	for dir in $(subdirs); do \
		$(MAKE) -C $$dir $@ ; \
	done
endif

# default target
all:: $(dest_par)

par:: $(dest_par)

# rebuild base par if target0 changes
$(dest_par): $(bin_target0) Makefile Cargo.toml
	@mkdir -p $(dir $(dest_par))
	$(WASH) par create \
		--arch $(ARCH_LOOKUP_$(par_target0)) \
		--binary $(bin_target0) \
		--capid $(CAPABILITY_ID) \
		--name $(NAME) \
		--vendor $(VENDOR) \
		--version $(VERSION) \
		--revision $(REVISION) \
		--destination $@ \
		--compress
	@echo Created $@

# par-full adds all the other targets to the base par
par-full: $(dest_par) $(bin_targets)
	for target in $(par_targets); do \
	    target_dest=target/$${target}/release/$(bin_name);  \
		if [ $$target = "x86_64-pc-windows-gnu" ]; then \
			target_dest=$$target_dest.exe;  \
		fi; \
	    par_arch=`printf $$target | sed -E 's/([^-]+)-([^-]+)-([^-]+)(-gnu.*)?/\1-\3/' | sed 's/darwin/macos/'`; \
		echo building $$par_arch; \
		if [ $$target_dest != $(cross_target0) ] && [ -f $$target_dest ]; then \
		    $(WASH) par insert --arch $$par_arch --binary $$target_dest $(dest_par); \
		fi; \
	done

# create rust build targets
ifeq ($(wildcard ./Cargo.toml),./Cargo.toml)

# rust dependencies
RUST_DEPS += $(wildcard src/*.rs) $(wildcard target/*/deps/*) Cargo.toml Makefile

target/release/$(bin_name): $(RUST_DEPS)
	cargo build --release

target/debug/$(bin_name): $(RUST_DEPS)
	cargo build

# cross-compile target, remove intermediate build artifacts before build
target/%/release/$(bin_name): $(RUST_DEPS)
	tname=`printf $@ | sed -E 's_target/([^/]+)/release.*$$_\1_'` &&\
	rm -rf target/release/build &&\
	cross build --release --target $$tname

endif

# rules to print file name and path of build target
target-path:
	@echo $(dest_par)
target-path-abs:
	@echo $(abspath $(dest_par))
target-file:
	@echo $(notdir $(dest_par))


# push par file to registry
push: $(dest_par)
	$(WASH) reg push $(oci_insecure) $(oci_url) $(dest_par)

# start provider
start:
	$(WASH) ctl start provider $(oci_url) \
		--host-id $(shell $(WASH) ctl get hosts -o json | jq -r ".hosts[0].id") \
		--link-name $(link_name) \
		--timeout-ms 4000

# inspect claims on par file
inspect: $(dest_par)
	$(WASH) par inspect $(dest_par)

inventory:
	$(WASH) ctl get inventory $(shell $(WASH) ctl get hosts -o json | jq -r ".hosts[0].id")


# clean: remove built par files, but don't clean if we're in top-level dir
ifeq ($(wildcard build/makefiles),)
clean::
	rm -rf build/
endif


ifeq ($(wildcard ./Cargo.toml),./Cargo.toml)
build::
	cargo build

release::
	cargo build --release

clean::
	cargo clean
	if command -v cross; then cross clean; fi

endif


install-cross: ## Helper function to install the proper `cross` version
	cargo install --git https://github.com/ChrisRx/cross --branch add-darwin-target --force


# for debugging - show variables make is using
make-vars:
	@echo "platform_id    : $(platform_id)"
	@echo "platform       : $(platform)"
	@echo "machine_id     : $(machine_id)"
	@echo "default-par    : $(par_target0)"
	@echo "project_dir    : $(project_dir)"
	@echo "subdirs        : $(subdirs)"
	@echo "top_targets    : $(top_targets)"
	@echo "NAME           : $(NAME)"
	@echo "VENDOR         : $(VENDOR)"
	@echo "VERSION        : $(VERSION)"
	@echo "REVISION       : $(REVISION)"


.PHONY: all par par-full test clean
