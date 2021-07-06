# examples/actor/Makefile

TOPTARGETS := all clean

SUBDIRS := $(wildcard */.)

$(TOPTARGETS):
	for dir in $(SUBDIRS); do \
        $(MAKE) -C $$dir $@; \
    done

.PHONY: $(TOPTARGETS) $(SUBDIRS)
