# Tools

## docker-compose
Bundles [NATS](https://hub.docker.com/_/nats/), [Redis](https://hub.docker.com/_/redis) and [Registry](https://hub.docker.com/_/registry) into a single manifest. These components are commonly used during wasmcloud development and when running our example components and providers, so it's beneficial to use this compose file when starting your wasmcloud journey.

## kvcounter-example
Helper script to run our [keyvalue counter](https://github.com/wasmcloud/examples/tree/master/kvcounter) component, [redis](https://github.com/wasmcloud/capability-providers/tree/main/redis) capability provider and [httpserver](https://github.com/wasmcloud/capability-providers/tree/main/http-server) capability providers. This example shows the interaction that an component can have with multiple capability providers, and serves as a sample reference for using `wash` in the CLI or in the REPL.

Running `bash kvcounter-example.sh` will attempt to determine if the program prerequisites are running (NATS, Redis, and a wasmcloud host) and then execute the following `wash` commands to launch and configure our components and providers.
```shell
wash ctl start component wasmcloud.azurecr.io/kvcounter:0.2.0
wash ctl start provider wasmcloud.azurecr.io/redis:0.10.0
wash ctl link MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E VAZVC4RX54J2NVCMCW7BPCAHGGG5XZXDBXFUMDUXGESTMQEJLC3YVZWB wasmcloud:keyvalue URL=redis://localhost:6379
wash ctl start provider wasmcloud.azurecr.io/httpserver:0.10.0
wash ctl link MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M wasmcloud:httpserver PORT=8080
```
If a wasmcloud host is not running, the script will simply output the above commands without the `wash` prefix, and indicate that you can run those commands in the `wash` REPL by running `wash up`. Running `wash up` will launch an interactive REPL environment that comes preconfigured with a wasmcloud host.
```
No hosts found, please run the wasmcloud binary, or proceed with the following commands in the REPL:

ctl start component wasmcloud.azurecr.io/kvcounter:0.2.0
ctl start provider wasmcloud.azurecr.io/redis:0.10.0
ctl link MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E VAZVC4RX54J2NVCMCW7BPCAHGGG5XZXDBXFUMDUXGESTMQEJLC3YVZWB wasmcloud:keyvalue URL=redis://localhost:6379
ctl start provider wasmcloud.azurecr.io/httpserver:0.10.0
ctl link MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M wasmcloud:httpserver PORT=8080
ctl call MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E HandleRequest {"method": "GET", "path": "/mycounter", "body": "", "queryString":"", "header":{}}
```