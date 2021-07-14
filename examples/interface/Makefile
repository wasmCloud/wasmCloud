# examples/interface/Makefile

TOPTARGETS := all build release clean lint validate

SUBDIRS := $(wildcard */.)

# find weld binary: (search order: environment (WELD), top folder debug build, PATH)
ifeq ($(WELD),)
 ifeq ($(wildcard ../../target/debug/weld),)
  ifeq ($(shell which weld 2>/dev/null),)
    $(error No weld in your PATH. try installing with 'cargo install weld-bin')
  else
    WELD=weld
  endif
 else
    WELD=$(realpath ../../target/debug/weld)
 endif
endif

.ONESHELL:
$(TOPTARGETS):
	for dir in $(SUBDIRS); do \
        $(MAKE) -C $$dir $@ WELD=$(WELD); \
    done

.PHONY: $(TOPTARGETS) $(SUBDIRS)
.NOTPARALLEL:
