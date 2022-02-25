# capability-providers/Makefile

subdirs = httpclient httpserver-rs kvredis kv-vault nats sqldb-postgres lattice-controller

include build/makefiles/recurse.mk
