# capability-providers/Makefile

subdirs = httpclient httpserver-rs kvredis nats sqldb-postgres

include build/makefiles/recurse.mk
