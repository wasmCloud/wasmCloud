

.PHONY: all build release clean

all: build

build::
	cargo build

release::
	cargo build --release

clean::
	cargo clean


build release clean::
	$(MAKE) -C examples $@
