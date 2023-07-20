#!/bin/sh -ex

rm -f dist/*.css
docker run --rm -it \
    -v "$PWD/../../codegen/templates/html:/templates:ro" \
    -v "$PWD:/project:rw" \
    -v "$OUT:/public:rw" \
    --user "$(id -u):$(id -g)" \
    --workdir /project \
    node:latest /project/build-tailwind-css.sh

mkdir -p ./gen/css
cp -v ./dist/*.css ./gen/css/



