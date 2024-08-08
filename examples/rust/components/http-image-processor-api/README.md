# HTTP Image Processor Component

This component receives and processes images (filtering, resizing, etc) images over a HTTP API.

The HTTP API is *similar* to projects like [Thumbor][thumbor] and [imagor][imagor], in that operations on images can be supplied via query parameters and as part of the URL. This component improves on the designs of those libraries by also operations to be specified as a JSON body.

This component is intended to works in concert with *another* component which contains *only* the functionality for image operations: the `image-processor` component. It is possible to combine these two components in two ways:

- Statically (creating a new binary) by *composing* the two WebAssembly binaries together
- Dynamically (using the [wasmCloud compute platform][wasmcloud]) by running wasmCloud (see [`wadm.yaml`](./wadm.yaml))

By running both this API component (`http-image-processor-api`) and the related `image-processor` separately, we can independently scale the two according to demand.

[thumbor]: https://github.com/thumbor/thumbor
[imagor]: https://github.com/cshum/imagor
[wasmcloud]: https://wasmcloud.com

## How does it work?

(( TODO: DIAGRAM ))

To create a working application, we must assemble a few parts:

- Incoming HTTP [capability provider][wasmcloud-docs-providers] for receiving incoming requests
- Outgoing HTTP [capability provider][wasmcloud-docs-providers] for sending HTTP requests
- A Blobstore (object storage) [capability provider][wasmcloud-docs-proivders] for storing and retrieving image data
- `http-image-processor-api` (this component) that receives those HTTP requests and interprets the API calls
- `image-processor` component which performs the actual transformations

> [!NOTE]
> Want to see how these pieces fit together? Check out [`wadm.yaml`](./wadm.yaml).

The flow for a request like `localhost:8080/rotate(x,y)/http://some/path/to/img.png` to the `http-image-processor-api` goes like this:

(( TODO: SEQUENCE DIAGRAM ))

[wasmcloud-docs-providers]: https://wasmcloud.com/docs/concepts/providers

## Prerequisites

- `cargo` >= 1.80
- [`wash`](https://wasmcloud.com/docs/installation) >= 0.30.0

Expecting more? the capability providers used along with this application are managed by wasmCloud and are downloaded from GitHub container registry (`ghcr.io`) on demand. See [`wadm.yaml`](./wadm.yaml) for more.

## Quickstart

To get started quickly, let's start wasmCloud host and deploy the manifest (`wadm.yaml`) in this folder directly. We'll use pre-prepared versions of this component and all it's dependencies.

```console
wash up --wadm-manifest wadm.yaml
```

### Running with your own changes

Want to make changes to this application and run with the code in this folder? see [`docs/local-build.md`](./docs/local-build.md).

## Use the application

Now that the application is running we can access `localhost:8080` and try it out, let's do a simple transformation on the [WebAssembly Logo](https://webassembly.org/css/webassembly.svg):

![Web Assembly logo](https://webassembly.org/css/webassembly.svg)

We can use a simple web request in the Thumbor/Imagor style:

```console
curl -LO "localhost:8080/process/resize:500x500/filter:grayscale()/https://webassembly.org/css/webassembly.svg"
```

Visiting this URL will:

- Fetch the image from `https://webassembly.org/css/webassembly.svg`
- Resize the image to 500 x 500 pixels
- Grayscale the image

By default, the data we get back is *the modified image* (named `output.svg`) -- so we save the image with the `curl`'s `-LO` option.

Thanks to `curl` you should now have a `output.png` file on your disk that looks like this:

(( TODO: output image ))

### Downloading an image from the internet

We can also download images from the internet -- let's do the same simple transformation on this wasmCloud visual:

![wasmCloud visual](https://wasmcloud.com/assets/images/wasmcloud-a-retro-3c4c36db4c29a6d738507d6a636ae25c.png)

We can use a simple web request in the Thumbor/Imagor style:

```console
curl -LO \
    "localhost:8080/process/resize:500x500/filter:grayscale()/https://wasmcloud.com/assets/images/wasmcloud-a-retro-3c4c36db4c29a6d738507d6a636ae25c.png"
```

Visiting this URL will:

- Fetch the image from `https://webassembly.org/css/webassembly.svg`
- Resize the image to 500 x 500 pixels
- Grayscale the image

By default, the data we get back is *the modified image* (named `output.svg`) -- so we save the image with the `curl`'s `-LO` option.

Thanks to `curl` you should now have a `output.png` file on your disk that looks like this:

### Uploading our own file

Let's say we have a file we want to upload and re-use, rather than always performing a web request to an external server. `http-image-processor-api` supports uploading files, thanks to the Blobstore provider that is connected (by default this uses the filesystem on the machine where the wasmCloud host is running).

We can use this picture of Terri:

![Terri the tardigrade Cosmonic mascot](./docs/terri.png)

We'll use *same* API, but this time we'll submit a form request (a `multipart/form-data` POST request, like a [`<form>`][mdn-form] on the web), along with chunked data in the body:

```console
curl -LO \
    "localhost:8080/process" \
    -F "operations[]=filter:fill(yellow)" \
    -F "operations[]=filter:saturation(100)" \
    -F "operations[]=resize:500x500" \
    -F "upload=true" \
    -F "upload-key=terri-png" \
    -F "image=@test/fixtures/terri.png"
```

This is mostly the same as our previous example except now we actually submit our operations as a list in the form, along with the option to perform an `upload`.

When we want to process the uploaded image, we can refer to it

```console
curl -LO \
    "localhost:8080/process" \
    -F "operations[]=filter:fill(yellow)" \
    -F "operations[]=filter:saturation(100)" \
    -F "operations[]=resize:500x500" \
    -F "upload-key=terri-png"
```

Now, the API will:

- Fetch the previously uploaded image (it might be cached)
- Perform the operations
- Return the updated image

As usual, you'll receive a `output.png`.

> [!NOTE]
> You can combine `upload` and an external URL! In that case the image will be fetched and uploaded to the connected blobstore.

### Using the API by POSTing JSON

Note that you can also use JSON:

```console
curl -LO \
    -X POST \
    "localhost:8080/process" \
    -H "Content-Type: application/json; charset=utf-8" \
    --data-binary @- <<EOF
{
  "image_source": {"type": "remote-https", "url": "https://path/to/image.png" },
  "image_format": "image/png",
  "blobstore_upload_original": { "link_name": "default", "bucket": "some-bucket", "key": "some/key/path" },
  "blobstore_upload_output": { "link_name": "default", "bucket": "some-bucket", "key": "some/other/key/path" },
  "operations": [
    {"type": "no-op"},
    {"type": "grayscale"},
    {"type": "resize", "height_px": 250, "width_px": 250}
  ]
}
EOF
```

> [!NOTE]
> You can exclude `blobstore_upload_original` and `blobstore_upload_output` to avoid uploading

[mdn-multipart-form]: https://developer.mozilla.org/en-US/docs/Web/HTML/Element/form
