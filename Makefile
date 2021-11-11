# capability-providers/Makefile

subdirs = httpclient httpserver-rs kvredis nats sqldb-postgres lattice-controller

include build/makefiles/recurse.mk
