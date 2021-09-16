top_targets     ?= all par par-full test clean

# traverse subdirs
.ONESHELL:
ifneq ($(subdirs),)
$(top_targets)::
	for dir in $(subdirs); do \
		$(MAKE) -C $$dir $@ ; \
	done
endif

.PHONY: all par par-full test clean
