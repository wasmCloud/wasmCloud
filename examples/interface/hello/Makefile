HTML_TARGET := html

CSS_FILES := tailwind.css tailwind.min.css
CSS_DEST  := $(addprefix $(HTML_TARGET)/css/,$(notdir $(CSS_FILES)))
CSS_URL   := https://wasmcloud.github.io/models/css
CONFIG    := ./codegen.toml

# find weld binary
ifeq ($(WELD),)
 ifeq ($(shell which weld 2>/dev/null),)
  $(error No weld in your PATH. try installing with 'cargo install weld-bin')
 else
  WELD=weld
 endif
endif


all: doc build

build release clean::
	$(MAKE) -C rust $@

clean::
	rm -rf $(HTML_TARGET)/*.html

# for docs, include BASE_MODELS so links work
doc: $(CSS_DEST)
	$(WELD) gen --lang html

# download any missing css files
$(HTML_TARGET)/css/%:
	mkdir -p $(dir $@)
	curl -L -o $@ "$(CSS_URL)/$(basename $@).css"

# Run lint or validate check on all smithy models
lint validate:
	$(WELD) $@

# view docs - after running this, open browser to localhost:8000
serve: doc
	python3 -m http.server -d $(HTML_TARGET) 8000

.PHONY: all build release lint validate serve clean
