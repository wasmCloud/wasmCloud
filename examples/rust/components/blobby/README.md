# Blobby

This component (we like to call it "Little Blobby Tables") is a simple file server showing the basic
CRUD operations of the `wasi:blobstore` contract.

Not only is this component an example, it is also a fully-functional, HTTP-based fileserver that can be
fronted with any HTTP server implementation and any blobstore implementation (i.e. you could store
the uploaded files on a filesystem or in an s3 compatible store).

## Prerequisites

- `cargo` 1.75
- [`wash`](https://wasmcloud.com/docs/installation) 0.26.0

## Building

```bash
wash build
```

## Running with wasmCloud

Ensuring you've built your component with `wash build`, you can launch wasmCloud and deploy the full hello world application with the following commands. Once the application reports as **Deployed** in the application list, you can use `curl` to send a request to the running HTTP server.

```shell
wash up -d
wash app deploy ./wadm.yaml
wash app get
curl http://localhost:8000
```

## Required Capabilities

1. `wasi:http` to receive http requests
2. `wasi:blobstore` to save the image to a blob
3. `wasi:logging` so the component can log

Once everything is up and running, you can run through all of the operations by following the
annotated commands below:

```console
# Create a file with some content
$ echo 'Hello there!' > myfile.txt

# Upload the file to the fileserver
$ curl -H 'Content-Type: text/plain' -v 'http://127.0.0.1:8000/myfile.txt' --data-binary @myfile.txt
*   Trying 127.0.0.1:8000...
* Connected to 127.0.0.1 (127.0.0.1) port 8000 (#0)
> POST /myfile.txt HTTP/1.1
> Host: 127.0.0.1:8000
> User-Agent: curl/7.85.0
> Accept: */*
> Content-Type: text/plain
> Content-Length: 12
>
* Mark bundle as not supporting multiuse
< HTTP/1.1 200 OK
< content-length: 0
< date: Wed, 18 Jan 2023 23:12:56 GMT
<
* Connection #0 to host 127.0.0.1 left intact

# Get the file back from the server
$ curl -v 'http://127.0.0.1:8000/myfile.txt'
*   Trying 127.0.0.1:8000...
* Connected to 127.0.0.1 (127.0.0.1) port 8000 (#0)
> GET /myfile.txt HTTP/1.1
> Host: 127.0.0.1:8000
> User-Agent: curl/7.85.0
> Accept: */*
>
* Mark bundle as not supporting multiuse
< HTTP/1.1 200 OK
< content-length: 13
< date: Wed, 18 Jan 2023 23:24:24 GMT
<
Hello there!
* Connection #0 to host 127.0.0.1 left intact

# Update the file
$ echo 'General Kenobi!' >> myfile.txt
$ curl -H 'Content-Type: text/plain' -v 'http://127.0.0.1:8000/myfile.txt' --data-binary @myfile.txt
*   Trying 127.0.0.1:8000...
* Connected to 127.0.0.1 (127.0.0.1) port 8000 (#0)
> POST /myfile.txt HTTP/1.1
> Host: 127.0.0.1:8000
> User-Agent: curl/7.85.0
> Accept: */*
> Content-Type: text/plain
> Content-Length: 29
>
* Mark bundle as not supporting multiuse
< HTTP/1.1 200 OK
< content-length: 0
< date: Wed, 18 Jan 2023 23:25:18 GMT
<
* Connection #0 to host 127.0.0.1 left intact

# Get the file again to see your updates
$ curl -v 'http://127.0.0.1:8000/myfile.txt'
*   Trying 127.0.0.1:8000...
* Connected to 127.0.0.1 (127.0.0.1) port 8000 (#0)
> GET /myfile.txt HTTP/1.1
> Host: 127.0.0.1:8000
> User-Agent: curl/7.85.0
> Accept: */*
>
* Mark bundle as not supporting multiuse
< HTTP/1.1 200 OK
< content-length: 29
< date: Wed, 18 Jan 2023 23:26:17 GMT
<
Hello there!
General Kenobi!
* Connection #0 to host 127.0.0.1 left intact

# Delete the file
$ curl -X DELETE -v 'http://127.0.0.1:8000/myfile.txt'
*   Trying 127.0.0.1:8000...
* Connected to 127.0.0.1 (127.0.0.1) port 8000 (#0)
> DELETE /myfile.txt HTTP/1.1
> Host: 127.0.0.1:8000
> User-Agent: curl/7.85.0
> Accept: */*
>
* Mark bundle as not supporting multiuse
< HTTP/1.1 200 OK
< content-length: 0
< date: Wed, 18 Jan 2023 23:33:02 GMT
<
* Connection #0 to host 127.0.0.1 left intact

# (Optional) See that the file doesn't exist anymore
$ curl -v 'http://127.0.0.1:8000/myfile.txt'
*   Trying 127.0.0.1:8000...
* Connected to 127.0.0.1 (127.0.0.1) port 8000 (#0)
> GET /myfile.txt HTTP/1.1
> Host: 127.0.0.1:8000
> User-Agent: curl/7.85.0
> Accept: */*
>
* Mark bundle as not supporting multiuse
< HTTP/1.1 404 Not Found
< content-length: 0
< date: Wed, 18 Jan 2023 23:39:07 GMT
<
* Connection #0 to host 127.0.0.1 left intact
```
