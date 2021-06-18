
HTML_TARGET = models/html

WELD = target/debug/weld
CSS_BUILD_OUT = $(wildcard docgen/dev/gen/css/*.css)
CSS_BUILD_SRC  = docgen/dev/src/css/styles.css
CSS_DEST = $(addprefix $(HTML_TARGET)/css/,$(notdir $(CSS_BUILD_OUT)))

MODEL_DIR = models/smithy
ALL_MODELS = $(wildcard models/smithy/*.smithy)
ALL_TEMPLATES = $(wildcard docgen/templates/*.hbs)

.PHONY: lint validate serve doc

# Run lint check on all smithy models in the models/smithy folder
lint:
	$(WELD) lint $(ALL_MODELS)

# Run validation checks on all smithy models in the models/smithy folder
validate:
	$(WELD) lint $(ALL_MODELS)

serve:
	python3 -m http.server -d $(HTML_TARGET) 8000

doc:
	# To generate docs, you don't need `--template-dir docgen/templates` because
	# templates are compiled into the weld binary. If you are doing development on the
	# templates, use `--template-dir` (and optionally `--template`) to override
	# the defaults, so you don't need to recompile `weld` to test new templates.
	$(WELD) doc --template-dir docgen/templates --output-dir $(HTML_TARGET) $(ALL_MODELS)


$(CSS_DEST): $(CSS_BUILD_OUT)
	mkdir -p $(dir $@)
	cp -p ${CSS_BUILD_OUT} $(HTML_TARGET)/css/

# rebuild tailwind.css if source or any templates change
# (pruning to generate .min.css is dependent on styles used in templates)
$(CSS_BUILD_OUT): $(CSS_BUILD_SRC) $(ALL_TEMPLATES)
	cd docgen/dev && ./update-css.sh
