# examples/Makefile

TOPTARGETS := all clean

SUBDIRS := interface actor provider

MANIFEST := manifest.toml
EX_LOADER := project.exs
EX_LOADER_DEPS := extra/loadactors.ex extra/make-project.sh Makefile $(MANIFEST)

# find weld binary: (search order: environment (WELD), parent folder debug build, PATH)
ifeq ($(WELD),)
 ifeq ($(wildcard ../target/debug/weld),)
  ifeq ($(shell which weld 2>/dev/null),)
    $(error No weld in your PATH. try installing with 'cargo install weld-bin')
  else
    WELD=weld
  endif
 else
    WELD=$(realpath ../target/debug/weld)
 endif
endif

.ONESHELL:
$(TOPTARGETS)::
	for dir in $(SUBDIRS); do \
        $(MAKE) -C $$dir $@ WELD=$(WELD); \
    done

# lint and validate only need to traverse interface, not all subdirs
lint validate::
	$(MAKE) -C interface $@ WELD=$(WELD)

all:: $(EX_LOADER)

# build project.exs: elixir project loader
$(EX_LOADER): $(wildcard */*/build/exs.out) $(EX_LOADER_DEPS)
	OUTPUT=$@ INPUT=$(MANIFEST) extra/make-project.sh

clean::
	rm -f $(EX_LOADER)

# just build one of the subdirectories
interface actors providers:
	$(MAKE) -C $@ all

.PHONY: $(TOPTARGETS) $(SUBDIRS)
.NOTPARALLEL: