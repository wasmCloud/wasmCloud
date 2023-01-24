# wasmCloud Roadmap and Vision

This document covers the current roadmap and vision for wasmCloud, as defined by the current project
and org maintainers. Each category below has more information on what each thing means. As we start
to define more of the items covered, we will link to the relevant issues. This may also continue to
evolve as we use things like GitHub milestones or other tools for tracking our progres. 

Overall, we want the community to have a good idea of where things are headed and what things they
can contribute to. To be clear, this list is not meant as the end all, be all of what we are doing
is wasmCloud. Not everything has to be on here to be considered "official work." It is meant more as
a [guideline](https://tenor.com/bcCX3.gif) for what the maintainers are thinking about. If you don't
see something on here that you think should be on here, feel free to [open a
PR!](https://github.com/wasmCloud/wasmCloud/pulls)

## Now

This category contains things we are currently (or will very soon be) working on. By definition it
is very narrow so you know exactly what is coming next 

- SQL V2 contract
  - We are trying to align with current webassembly standards for this contract
  - Mitigate the possibility of injection attacks
- Finishing up [wadm](https://github.com/wasmCloud/wadm)
  - Validating design assumptions
  - Supporting custom scaling algorithms
  - Deployment and infra docs
  - Getting started and advanced guides and examples
  - Removing dependency on Redis (NATS only if possible)

## Next

The Next category contains items we consider important to work on after we finish the work in the
[Now](#now) section. These are not ordered in any way, but indicate that we want to actually get
these done relatively soon. If any of these interest you and you'd like to help, please let us know!

- Doc for adding each capability, e.g.
  - examples/how-to-add-logging
- KeyValue contract v2
- Streaming or some sort of workaround for large file handling
- Messaging contract v2
- Go SDK
- Linkdefs 2.0: Make things easy to configure
- Completely revamp wasmbus rpc
  - Split out into actor and provider SDKs, fix things like owned vs borrowed data, make initting
    logging better, etc.

## Ideas List

This category comes with no promises. It serves as a list of ideas we really want to implement, but
we haven't fully spec'd out. Most of them are larger chunks of work that could be broken down a bit
as we move them into the [Next](#next) section.

- JavaScript Actors (probably with QuickJS)
- Unit tests for actors (of any language preferably)
- Components implementation and SDK
- JS host
- Embedded-first host
- Multi-tenant wasmcloud host