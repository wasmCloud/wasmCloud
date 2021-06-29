
HTML_TARGET = models/html

WELD_DBG = target/debug/weld
WELD_REL = target/release/weld
WELD     = $(WELD_DBG)
CSS_BUILD_OUT = $(wildcard docgen/dev/gen/css/*.css)
CSS_BUILD_SRC  = docgen/dev/src/css/styles.css
CSS_DEST = $(addprefix $(HTML_TARGET)/css/,$(notdir $(CSS_BUILD_OUT)))

ALL_MODELS    = $(wildcard models/smithy/*.smithy)
ALL_TEMPLATES = $(wildcard docgen/templates/*.hbs) $(wildcard codegen/templates/**/*.hbs)

.PHONY: all build doc rust lint validate serve

all: $(WELD) build doc

$(WELD_DBG): $(ALL_TEMPLATES) bin/src/main.rs
	cargo build --package wasmcloud-weld-bin

$(WELD_REL): $(ALL_TEMPLATES) bin/src/main.rs
	cargo build --release --package wasmcloud-weld-bin

build: rust
	cargo build

doc: $(CSS_DEST)
	$(WELD) gen -l html --config codegen.toml \
		--template-dir=docgen/templates \
		--output-dir=. $(ALL_MODELS)

rust:
	$(WELD) gen -l rust --config codegen.toml \
		--template-dir=codegen/templates/rust \
		--output-dir=. $(ALL_MODELS)

# Run lint check on all smithy models in the models/smithy folder
lint:
	$(WELD) lint $(ALL_MODELS)

# Run validation checks on all smithy models in the models/smithy folder
validate:
	$(WELD) validate $(ALL_MODELS)

serve: doc
	python3 -m http.server -d $(HTML_TARGET) 8000

$(CSS_DEST): $(CSS_BUILD_OUT)
	mkdir -p $(dir $@)
	cp -p ${CSS_BUILD_OUT} $(HTML_TARGET)/css/

# rebuild tailwind.css if source or any templates change
# (pruning to generate .min.css is dependent on styles used in templates)
$(CSS_BUILD_OUT): $(CSS_BUILD_SRC) $(ALL_TEMPLATES)
	cd docgen/dev && ./update-css.sh

clean:
	cargo clean
	rm -f models/html/*.html
	#rm -f rpc/src/{core,health,model}.rs
