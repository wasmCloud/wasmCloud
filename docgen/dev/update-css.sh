#!/bin/sh

OUT=$(mktemp -d)
mkdir $OUT/css
chmod 755 $OUT
chmod 777 $OUT/css

docker run --rm \
    -v $PWD/../templates:/templates:ro \
    -v $PWD/conf:/project/conf:ro \
    -v $PWD/src:/project/src:ro \
    -v $OUT:/public \
    css-dev npm run build-all

mkdir -p ./gen/css
cp -v $OUT/css/*.css ./gen/css/



