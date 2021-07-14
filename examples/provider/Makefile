# examples/provider/Makefile

TOPTARGETS := all clean

#SUBDIRS := $(wildcard ./*/)
# avoid building others for now
SUBDIRS := hello httpserver

.ONESHELL:
$(TOPTARGETS):
	for dir in $(SUBDIRS); do \
        $(MAKE) -C $$dir $@ WELD=$(WELD); \
    done

.PHONY: $(TOPTARGETS) $(SUBDIRS)
.NOTPARALLEL:
