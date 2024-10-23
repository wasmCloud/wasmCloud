# Securing your wasmCloud Supply Chain with SBOMs

[SBOMs](https://edu.chainguard.dev/open-source/sbom/what-is-an-sbom/) ("Software Bill of Materials") are rapidly gaining adoption in the industry as a way to describe the set of software dependencies that are used as part of building your application.

When compared to some of the other software artifacts used for distributing and deploying applications in the Cloud Native ecosystem, WebAssembly modules and components have the distinct advantage of bringing nothing else except for the direct dependencies needed for building the application itself.

In this document, we will cover the tooling and minimum number of steps necessary in order for you to get started with building and verifying SBOMs for your wasmCloud applications.

## Pre-requisite tools:

In addition to the wasmCloud tooling (`wash`) and language-specific tooling described in the [wasmCloud Quickstart](https://wasmcloud.com/docs/tour/hello-world?lang=tinygo), you will need the following tools:

* [`syft`](https://github.com/anchore/syft?tab=readme-ov-file#installation)
* [`grype`](https://github.com/anchore/grype?tab=readme-ov-file#installation) (and optionally [`trivy`](https://github.com/aquasecurity/trivy?tab=readme-ov-file#get-trivy))

## tl;dr:

Using a brand new component example from [Quickstart](https://wasmcloud.com/docs/tour/hello-world#create-a-new-component):

1. To create a new component for testing SBOM generation, run `wash new component hello ...` (from the quickstart mentioned above).
2. Navigate into the newly created `hello` directory with `cd hello`
3. To update your local depedenencies, run `wash build`
4. To generate SBOM, run `syft scan dir:. --output spdx-json=sbom.spdx.json`
5. To scan your SBOM for vulnerabilities, run `grype sbom.spdx.json`
