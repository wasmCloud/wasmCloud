# examples/interface/Makefile

TOPTARGETS := all build release clean

SUBDIRS := $(wildcard */.)

$(TOPTARGETS):
	for dir in $(SUBDIRS); do \
        $(MAKE) -C $$dir $@; \
    done

.PHONY: $(TOPTARGETS) $(SUBDIRS)
