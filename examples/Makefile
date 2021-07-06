# examples/Makefile

TOPTARGETS := all clean

SUBDIRS := interface actor provider

MODULE := project.exs

$(TOPTARGETS)::
	for dir in $(SUBDIRS); do \
        $(MAKE) -C $$dir $@; \
    done

all:: $(MODULE)

#$(MODULE):
#	@# run once to make sure all dependencies are built
#	@$(MAKE) --quiet iex_vars > /dev/null
#	@$(MAKE) --quiet iex_vars > $@

.ONESHELL:
$(MODULE): $(SUBDIRS) Makefile $(wildcard */*/build/exs.out)
	@cat <<- EOF > $@
	defmodule Project do
		def defines() do
			%{ :actors => [
			$(file < actor/hello-send/build/exs.out)
			], :providers => [
			$(file < provider/httpserver/build/exs.out)
			$(file < provider/hello/build/exs.out)
			]}
		end
	end
	EOF

clean::
	rm -f $(MODULE)

.PHONY: $(TOPTARGETS) $(SUBDIRS) iex_vars
