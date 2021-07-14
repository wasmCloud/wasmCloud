HTML_TARGET := html

CSS_BUILD_OUT = $(wildcard ../../../docgen/dev/gen/css/*.css)
CSS_DEST = $(addprefix $(HTML_TARGET)/css/,$(notdir $(CSS_BUILD_OUT)))
CONFIG := ./codegen.toml

# find weld binary: (search order: environment (WELD), top folder debug build, PATH)
ifeq ($(WELD),)
 ifeq ($(wildcard ../../../target/debug/weld),)
  ifeq ($(shell which weld 2>/dev/null),)
    $(error No weld in your PATH. try installing with 'cargo install weld-bin')
  else
    WELD=weld
  endif
 else
    WELD=$(realpath ../../../target/debug/weld)
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

# Run lint or validation checks
lint validate:
	$(WELD) $@

# view docs - after running this, open browser to localhost:8000
serve: doc
	python3 -m http.server -d $(HTML_TARGET) 8000

# update css files in html/css
$(CSS_DEST): $(CSS_BUILD_OUT)
	mkdir -p $(dir $@)
	cp -p ${CSS_BUILD_OUT} $(HTML_TARGET)/css/

.PHONY: all build release lint validate serve clean
