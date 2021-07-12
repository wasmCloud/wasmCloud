HTML_TARGET := html

WELD := ../../../target/debug/weld
CSS_BUILD_OUT = $(wildcard ../../../docgen/dev/gen/css/*.css)
CSS_DEST = $(addprefix $(HTML_TARGET)/css/,$(notdir $(CSS_BUILD_OUT)))
CONFIG := ./codegen.toml

all: doc
	$(WELD) gen --lang rust $(ALL_MODELS) $(BASE_MODELS)

clean::
	rm -rf $(HTML_TARGET)/*.html

# for docs, include BASE_MODELS so links work
doc: $(CSS_DEST)
	$(WELD) gen --lang html $(ALL_MODELS) $(BASE_MODELS)

# Run lint check on all smithy models in the models/smithy folder
lint:
	$(WELD) lint $(ALL_MODELS) $(BASE_MODELS)

# Run validation checks on all smithy models in the models/smithy folder
validate:
	$(WELD) validate $(ALL_MODELS) $(BASE_MODELS)

# view docs - after running this, open browser to localhost:8000
serve: doc
	python3 -m http.server -d $(HTML_TARGET) 8000

# update css files in html/css
$(CSS_DEST): $(CSS_BUILD_OUT)
	mkdir -p $(dir $@)
	cp -p ${CSS_BUILD_OUT} $(HTML_TARGET)/css/

.PHONY: all build release lint validate serve clean
