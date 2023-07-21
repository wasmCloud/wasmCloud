#!/bin/sh

#source .env

NETWORK=examples_default

export PGPASSWORD="$POSTGRES_PASSWORD"

docker run -it --rm --network $NETWORK --link db --env-file .env \
  postgres:13 psql --host db -U postgres -d example $@

#    --host=db --port=5432 --username=postgres \
