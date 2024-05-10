# Step 0: Show running in wasmcloud
cd http-hello
wash build

# See wit world
wasm-tools component wit build/http_hello_world_s.wasm
wash app deploy wadm.yaml

wash app undeploy rust-hello-world

# Step 1: running in wasmtime

wasmtime serve -S cli=y build/http_hello_world.wasm

# Step 2: Add a custom pong interface
cd pong
wash build

wasm-tools component wit ./build/pong_s.wasm

wasi-virt build/pong_s.wasm --allow-random -e PONG=sw2con -o virt.wasm
wasm-tools component wit virt.wasm 

cd http-hello2
wash build

wash app deploy wadm.yaml

# Step 6: Composing the component for wasmtime

# Try running wasmtime serve without composition
wasmtime serve -S cli=y build/http_hello_world.wasm

cd .. 
wac encode --dep ping:pong=./pong/virt.wasm --dep hello:there=./http-hello2/build/http_hello_world_s.wasm -o output.wasm compose.wac
wash app undeploy rust-hello-world
wasmtime serve -S cli=y output.wasm
