CORE_TARGET = ../../models/docs/org.wasmcloud

TARGETS =  \
	$(CORE_TARGET)/wasmcloud-core.smithy \
	$(CORE_TARGET)/wasmcloud-model.smithy

deploy: $(TARGETS)


$(CORE_TARGET)/%: smithy/%
	cp -p $< $@
