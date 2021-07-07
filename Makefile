# weld top-level Makefile

all clean:: build
	$(MAKE) -C examples $@

build::
	cargo build

release::
	cargo build --release

clean test::
	cargo $@


.PHONY: all build release clean test
.NOTPARALLEL:
