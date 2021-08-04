# provider.mak
#
# common rules for building capability providers
# Some of these may depend on GNUMakefile >= 4.0
#
# before including this, local project makefile should define the following
# (to override defaults)
# top_targets      # list of targets that are applicable for this project
#

top_targets     ?= all build par clean

platform_id = $$( uname -s )
platform = $$( \
	case $(platform_id) in \
		( Linux | Darwin ) echo $(platform_id) ;; \
		( * ) echo Unrecognized Platform;; \
	esac )

machine_id = $$( uname -m )

# name of compiled binary
bin_name ?= $(PROJECT)
dest_par ?= build/$(bin_name).par.gz

# oci url generation assumes that vesion is first two parts of semver: x.y
oci_url_base ?= localhost:5000/v1
oci_url ?= $(oci_url_base)/$(bin_name):$(VERSION).$(REVISION)
ifeq ($(WASH_REG_USER),)
	oci_insecure := --insecure
endif

par_targets ?= \
	x86_64_unknown-linux \
   	x86_64-apple-darwin \
	armv7-unknown-linux-gnueabihf \
   	aarch64-unknown-linux-gnu \
   	aarch64-apple-darwin \
   	x86_64-pc-windows-gnu

bin_targets = $(foreach target,$(par_targets),target/$(target)/release/$(bin_name))

# pick target0 for starting par
ifeq ($(platform_id)_$(machine_id),Linux_x86_64)
	par_target0=x86_64_unknown-linux
else
	ifeq ($(platform_id)_$(machine_id),Darwin_x86_64)
	    par_target0=x86_64-apple-darwin
	else
		# default to linux-x86
	    par_target0=x86_64_unknown-linux
    endif
endif

# the target of the current platform, as defined by cross
cross_target0=target/$(par_target0)/release/$(bin_name)
# bin_target0=$(cross_target0)
bin_target0=target/release/$(bin_name)

# traverse subdirs
.ONESHELL:
ifneq ($(subdirs),)
$(top_targets)::
	for dir in $(subdirs); do \
		$(MAKE) -C $$dir $@ weld=$(weld); \
	done
endif

# this target must be listed first so that including this makefile
# doesn't trigger other rules
#all::

# build par file for current platform
par: release $(dest_par) build/stub.exs

# rebuild base par if target0 changes
$(dest_par): $(bin_target0) Makefile
	@mkdir -p $(dir $(dest_par))
	rm -f $@
	wash par create \
		--arch $(par_target0) \
		--binary $(bin_target0) \
		--capid $(CAPABILITY_ID) \
		--name $(NAME) \
		--vendor $(VENDOR) \
		--version $(VERSION) \
		--revision $(REVISION) \
		--destination $@ \
		--compress


# par-full adds all the other targets to the base par
par-full: $(dest_par) $(bin_targets)
	# add other defined targets
	for target in $(par_targets); do \
	    target_dest=target/$$target/release/$(bin_name);  \
		echo building $$target; \
		if [ $${target_dest} != $(cross_target0) ] && [ -f $$target_dest ]; then \
		    wash par insert --arch $$target --binary $$target_dest $@; \
		fi; \
	done

# create rust build targets
ifeq ($(wildcard ./Cargo.toml),./Cargo.toml)

# rust dependencies
RUST_DEPS += $(wildcard src/*.rs) Cargo.toml Makefile

target/release/$(bin_name): $(RUST_DEPS)
	cargo build --release

target/debug/$(bin_name): $(RUST_DEPS)
	cargo build

target/%/release/$(bin_name): $(RUST_DEPS)
	tname=`echo -n $@ | sed -E 's_target/([^/]+)/release.*$$_\1_'` &&\
	cross build --release --target $$tname

endif

# make exs stub for provider
build/stub.exs: $(dest_par)
	@mkdir -p $(dir $@)
	@cat <<- EOF > $@
	%{
		name: $(NAME),
		path: "$(abspath $(dest_par))",
		key: "$(shell wash par inspect $(dest_par) -o json | jq -r ".service")",
		link: "default",
		contract: "$(CAPABILITY_ID)",
	},
	EOF


# is this needed for cap providers?
## find weld binary: (search order: environment (weld), PATH)
#ifeq ($(weld),)
#	ifeq ($(shell which weld 2>/dev/null),)
#		$(error No weld in your PATH. try installing with 'cargo install weld-bin')
#	else
#		weld:=weld
#	endif
#endif


# push par file to registry
push: $(dest_par)
	wash reg push $(oci_insecure) $(oci_url) $(dest_par)

load:
	wash ctl start provider -o json $(oci_url)

# inspect claims on par file
inspect: $(dest_par)
	wash par inspect $(dest_par)


clean::
	rm -f build/*.par.gz build/*.exs

ifeq ($(wildcard ./Cargo.toml),./Cargo.toml)
build::
	cargo build

release::
	cargo build --release

clean::
	cargo clean
	cross clean
endif



# auto rust dependencies
# TODO
#   target: $(rust_deps)
#     $(cargo) build --target $(...)




# for debugging - show variables make is using
make-vars:
	@echo "weld:          : $(weld)"
	@echo "platform_id    : $(platform_id)"
	@echo "platform       : $(platform)"
	@echo "machine_id     : $(machine_id)"
	@echo "project_dir    : $(project_dir)"
	@echo "subdirs        : $(subdirs)"
	@echo "top_targets    : $(top_targets)"
	@echo "NAME           : $(NAME)"
	@echo "VENDOR         : $(VENDOR)"
	@echo "VERSION        : $(VERSION)"
	@echo "REVISION       : $(REVISION)"


.PHONY: all build release par clean test $(weld)
