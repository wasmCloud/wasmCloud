# interface/http-server/Makefile

HTML_TARGET := html
WELD := ../../../target/debug/weld
CSS_BUILD_OUT = $(wildcard ../../../docgen/dev/gen/css/*.css)
CSS_DEST = $(addprefix $(HTML_TARGET)/css/,$(notdir $(CSS_BUILD_OUT)))
CONFIG := ./codegen.toml


all: doc build

build release clean::
	$(MAKE) -C rust $@

clean::
	rm -rf $(HTML_TARGET)/*.html

# for docs
doc: $(CSS_DEST)
	$(WELD) gen --lang html --config $(CONFIG) --output-dir .

# Run lint check on all smithy models in the models/smithy folder
lint:
	$(WELD) lint --config $(CONFIG)

# Run validation checks on all smithy models in the models/smithy folder
validate:
	$(WELD) validate --config $(CONFIG)

# view docs - after running this, open browser to localhost:8000
serve: doc
	python3 -m http.server -d $(HTML_TARGET) 8000

# update css files in html/css
$(HTML_TARGET)/css/%: ../../../docgen/dev/gen/css/%
	cp -p $< $@

.PHONY: all build release lint validate serve clean
