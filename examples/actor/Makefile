# examples/actor/Makefile

SUBDIRS := $(wildcard */.)
TOPTARGETS := all clean

.ONESHELL:
$(TOPTARGETS):
	for dir in $(SUBDIRS); do \
        $(MAKE) -C $$dir $@ WELD=$(WELD); \
    done

.PHONY: $(TOPTARGETS) $(SUBDIRS)
.NOTPARALLEL:
