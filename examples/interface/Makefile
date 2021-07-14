# examples/interface/Makefile

TOPTARGETS := all build release clean

SUBDIRS := $(wildcard */.)

$(TOPTARGETS):
	for dir in $(SUBDIRS); do \
        $(MAKE) -C $$dir $@ WELD=$(WELD); \
    done

.PHONY: $(TOPTARGETS) $(SUBDIRS)
.NOTPARALLEL:
