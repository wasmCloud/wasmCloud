#!/bin/sh -x
# This is run inside the docker container created by update-css.sh

npm install
npm run build
npm run build-prod

