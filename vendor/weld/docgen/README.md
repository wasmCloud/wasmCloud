HTML templates and stylesheets for smithy model html documentation.

This folder contains 
- handlebars templates for generation of HTML documentation from smithy models (used by `weld gen -l html`)
- a nodejs project for building tailwind.css and tailwind.min.css. The .min version is created by removing styles not needed by our templates
- a docker container for building tailwind.css+tailwind.min.css that doesn't use any local nodejs or libraries

To update css files, first create the docker container
```
cd docgen/dev
./build-docker.sh
```

Then generate updated css files
```text
./update-css.sh
```

The generated files will be in `docgen/dev/gen/css/*.css` and should be copied to any output folder where you want to browse model documentation.

The structure of a documentation folder is
- `<output-dir>/html/*.html`   (generated)
- `<output-dir>/css/*.css`   (copied from css build dir)
- `<output-dir>/index.html`  (manually created)

It is on the roadmap to generate index.html from a template, but for now it must be created by hand.

