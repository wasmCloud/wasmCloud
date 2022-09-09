# capability-providers/Makefile

subdirs = blobstore-s3 blobstore-fs httpclient httpserver-rs kvredis kv-vault nats sqldb-postgres lattice-controller

include build/makefiles/recurse.mk