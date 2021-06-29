
# Getting Started

First, Make sure you have all prerequisites installed, including `weld`. The [prerequisites](./prerequisites.md) page has a list of what you need and where to find it.


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



# Creating an interface project

An interface project is a shared library that wraps one or more smithy models.

```text
weld gen --create interface  [OPTIONS]
```

- optional arguments
  - `--output-dir DIR`      - create the file in DIR (default ".")
  - `-D project_name=PROJ`  - initial project name (default "my_project")
