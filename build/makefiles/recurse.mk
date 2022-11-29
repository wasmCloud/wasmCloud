top_targets     ?= all par par-full test clean

# traverse subdirs
.ONESHELL:
ifneq ($(subdirs),)
$(top_targets)::
	for dir in $(subdirs); do \
		$(MAKE) -C $$dir $@ ; \
	done
endif

cargo-update:
	for dir in $(subdirs); do \
		(cd $$dir && cargo update) ; \
	done

.PHONY: all par par-full test clean
