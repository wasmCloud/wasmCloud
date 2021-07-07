# examples/Makefile

TOPTARGETS := all clean

SUBDIRS := interface actor provider

MANIFEST := manifest.toml
EX_LOADER := project.exs
EX_LOADER_DEPS := extra/loadactors.ex extra/make-project.sh Makefile $(MANIFEST)

$(TOPTARGETS)::
	for dir in $(SUBDIRS); do \
        $(MAKE) -C $$dir $@; \
    done

all:: $(EX_LOADER)

$(EX_LOADER): $(wildcard */*/build/exs.out) $(EX_LOADER_DEPS)
	OUTPUT=$@ INPUT=$(MANIFEST) extra/make-project.sh

clean::
	rm -f $(EX_LOADER)

.PHONY: $(TOPTARGETS) $(SUBDIRS)
.NOTPARALLEL: