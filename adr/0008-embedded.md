# wasmCloud Will Not Run on Embedded Devices w/out Linux

This decision record covers our decision to **not** create our own fully supported wasmCloud host runtime for embedded devices that do not run on one of our already support targets (e.g. `arm` or `aarch64` etc).

## Context and Problem Statement

One of the main value propositions of the wasmCloud lattice is that it can connect actors and capability providers running _anywhere_. If that's true, then we must consider what it would take to run providers and actors on tiny embedded devices like asset trackers, sensor equipment, telemetry gathering devices, weather stations, maker boards, and the like.

## Considered Options

There are a few options that we considered:

* Create our own official first-party distribution for all supported devices
* Do not create a first-party distribution for embedded devices
* Provide guidance to capability provider developers on how to act as an "MQTT bridge" to remote embedded devices.

## Decision Outcome

We chose the last option, _provide guidance to capability provider developers on how to act as an "MQTT bridge" to remote embedded devices_. This will be resolved through a separate PR to our documentation site.

## Pros and Cons of the Options

In [the related RFC issue](https://github.com/wasmCloud/wasmCloud/issues/194), we discuss all of the pros and cons of the various options. In short, it would be impractical and far too time and resource consuming for us to create releases and custom libraries for each combination of hardware and chipsets. Instead, offering the option of the "IoT proxy provider" seems like the best approach that keeps us focused on the cloud native runtime that allows actors to "run anywhere."
