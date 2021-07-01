# http-server/Makefile

HTML_TARGET = html

WELD = ../../../target/debug/weld
CSS_BUILD_OUT = $(wildcard ../../../docgen/dev/gen/css/*.css)
CSS_DEST = $(addprefix $(HTML_TARGET)/css/,$(notdir $(CSS_BUILD_OUT)))

.PHONY: all build release lint validate serve clean

all: doc build

build:
	@cd rust && cargo build

release:
	@cd rust && cargo build --release

clean:
	rm -rf $(HTML_TARGET)/*.html
	@cd rust && cargo clean

# for docs
doc: $(CSS_DEST)
	$(WELD) gen --lang html --config rust/codegen.toml --output-dir ..

# Run lint check on all smithy models in the models/smithy folder
lint:
	$(WELD) lint --config rust/codegen.toml

# Run validation checks on all smithy models in the models/smithy folder
validate:
	$(WELD) validate --config rust/codegen.toml

# view docs - after running this, open browser to localhost:8000
serve: doc
	python3 -m http.server -d $(HTML_TARGET) 8000

# update css files in html/css
$(HTML_TARGET)/css/%: ../../../docgen/dev/gen/css/%
	cp -p $< $@