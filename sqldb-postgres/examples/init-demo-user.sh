#!/bin/bash

# sample script to show how to create a demo user and database
# you'll need postgres cli tools 'psql' and 'pg_restore'

set -e
source .env

psql -v ON_ERROR_STOP=1 -h 127.0.0.1 -p 5432 -U postgres --dbname "$POSTGRES_DB" <<-EOSQL
    CREATE USER demo WITH PASSWORD 'demo';
    CREATE DATABASE dvdrental;
    GRANT POSTGRES TO demo
    GRANT ALL PRIVILEGES ON DATABASE dvdrental TO demo;
EOSQL

# for sample data (the 'dvdrental' database), download from
# https://www.postgresqltutorial.com/postgresql-sample-database/
# and load with the following command:
# pg_restore -h 127.0.0.1 -p 5432 -U demo  -d dvdrental dvdrental.tar


