# examples/actor/Makefile

SUBDIRS := $(wildcard */.)
TOPTARGETS := all clean

$(TOPTARGETS):
	for dir in $(SUBDIRS); do \
        $(MAKE) -C $$dir $@ WELD=$(WELD); \
    done

.PHONY: $(TOPTARGETS) $(SUBDIRS)
.NOTPARALLEL:
