
# Getting Started

First, Make sure you have all prerequisites installed, including `weld`. The [prerequisites](./prerequisites.md) page has a list of what you need and where to find it.

- [Creating an actor](#creating-an-actor)
- [Creating an interface](#creating-an-interface-project)
- [Model builds and debugging](#model-builds-and-debugging)



## Creating an actor


```text
weld gen --create actor  [OPTIONS]
```

- optional arguments
    - `--output-dir DIR`      - create the file in DIR (default ".")
    - `-D project_name=PROJ`  - initial project name (default "my_project")

The actor project creates an echo actor that implements the httpserver interface. When it receives an http message, it echoes back data from the request.

To build, 
```text
make
```

To run

```
make run
```

You should see the debug log from wasmcloud, with a line near the end containing
```
Starting "actix-web-service-0.0.0.0:8080" service on 0.0.0.0:8080A
```

If you get an error "Invalid HttpServer bind address" ... "Address already in use" (or similar), you may need to change the port number from 8080. If this error occurs, stop wasmcloud with ctrl-c, and change the port in `manifest.yaml`. You'll need to use the new port number in the curl command below. Restart wasmcloud with `make run`.

In another terminal, you should be able to invoke the actor with
```
curl localhost:8080/123
```

and you should get a response:
```
{"body":[120,121,122],"method":"GET","path":"/123","query_string":""}
```

Congratulations! You build an actor, launched a wasmcloud, and invoked the actor over HTTP.



## Creating an interface project

An interface project is a shared library that wraps one or more smithy models.

```text
weld gen --create interface  [OPTIONS]
```

- optional arguments
  - `--output-dir DIR`      - create the file in DIR (default ".")
  - `-D project_name=PROJ`  - initial project name (default "my_project")


## Model builds and debugging

An interface project is the only kind of project that needs Smithy model files. The interface project generates code to build libraries that are linked by actor projects and capability-provider projects.

In your interface project, there should be a [`codegen.toml`](./codegen-toml.md) in the current directory, containing paths to your project model and any dependencies that might look like this:

```text
# model sources
[[models]]
path = "."
files = [ "http-server.smithy" ]

# wasmcloud core dependencies
[[models]]
url = "https://wasmcloud.github.io/models/org.wasmcloud"
files = [ "wasmcloud-core.smithy", "wasmcloud-model.smithy" ]

```

In the current directory, you should be able to run `weld lint` and `weld validate` and get no errors.

### Error messages

Error messages generated with `weld lint` and `weld validate` are fairly detailed and should be self-explanatory.

If there is a syntax error in one of the IDL files, the error message is somewhat long, but the most useful part is `line_col: Pos[L,C]`, where L and C are the line number and column number of the syntax error. Shortly after that there should be a `line:` showing the line of the input file containing the error.


### Clippy warning on `&String` parameters

If your `.smithy` model has an operation whose input parameter is a 'String', clippy may generate the following warning:
```
warning: writing `&String` instead of `&str` involves a new object where a slice will do
```

That's incorrect for our use case (try changing it and you'll see that change `&String` to `&str` doesn't work). To avoid this warning, add `#![allow(clippy::ptr_arg)]` as the first line in your interface project's src/lib.rs (or whichever source file includes the generated code from OUT_DIR).


### Where is the generated code?

The code generated for rust doesn't go into the rust/src folder, it gets generated into a build folder under `target`. (The actual directory is set by the compiler and passed to the build script as the environment variable `OUT_DIR`.)

One easy way to view the generated rust source code is to generate it into a known directory. The command `weld gen -o /tmp/project` puts the code into `/tmp/project/src/`.

The build is optimized so that model parsing and code generation is only done if there is a change to `codegen.toml` or one of the model files.


