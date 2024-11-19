HELLO_WASM=http_hello_world.wasm
WASI_SDK_DIR?=/opt/wasi-sdk
AR=$(WASI_SDK_DIR)/bin/ar
CC=$(WASI_SDK_DIR)/bin/clang
VERSION=0.1.0

$(HELLO_WASM): hello_component_type.o hello_world.c
	$(CC) -target wasm32-wasip2 -mexec-model=reactor -Wunused hello.c hello_world.c hello_component_type.o -o http_hello_world.wasm

hello_component_type.o:
	wit-bindgen c --autodrop-borrows yes ./wit

clean:
	rm -f *.o *.a *.h *.wasm hello.c
