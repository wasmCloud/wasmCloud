# wasmCloud Benchmarking and Performance Numbers

_Last updated 03/11/2024_

This document contains some basic benchmarking tests to give us a theoretical max throughput of
requests per second. Ideally, these benchmarks will eventually happen automatically as part of a
built in suite of tests.

The purpose of these numbers is to give users a starting point for capacity planning when running
wasmCloud.

## Caveats, Provisos, Particulars, Stipulations, etc.

As with any benchmark, it is extremely important to know what is being tested and under what
conditions it is run. With that in mind, here are some important details:

- These tests were all run with a simple Rust Hello World component (the same one you get if you run
  `wash new actor`) returning the text "Hello from Rust!" It performs no other operations. This
  means this is a semi-decent proxy for the maximum throughput.
- All tests used the command `hey -z 20s -c 150 http://localhost:8080`
- We ran tests on a 10 core M1 Max processor with 64GB of RAM running MacOS Sonoma 14.3.1 and on a
  cloud VM with a 16 Core AMD Rome processor and 64 GB of RAM running Debian Bookworm with kernel
  version 6.1. The numbers were sometimes _slightly_ higher on the Linux machine, but were for the
  most part the same.
  - Another test we haven't run, but would like to, is on a Raspberry Pi 4 and/or Raspberry Pi Zero
    2. This would probably give us a good guess at the theoretical minimum throughput
- This test used optimizations that are not yet available in a released version, but will be part of
  the 1.0 release. Namely: a locking issue in the internal request handling logic of a wasmCloud
  host, removing default request tracing from the HTTP Server Provider, and using the Pooled Memory
  Allocator available in wasmtime.
- As part of our change to use wRPC as part of the 1.0 release, initial smoke tests are actually
  showing an _increase_ in throughput. We will try these tests again once we've cut 1.0
- Because a "Hello World" component runs so quickly, it didn't actually matter how many of them we
  allowed to run simultaneously. So this test does not give us any data on tweaking the
  `max_concurrent` value when running a component

Also, just because people always ask: _NATS is not the bottleneck_, nor will it likely be the
bottleneck for the near-to-medium term future (possibly even ever). When running the `nats bench`
tool on the same machines we got these numbers on (using the request/response pattern), the
throughput was absurdly high. If you don't believe us, go spin up a similar sized node to the ones
used in the test and then run `nats bench -h` for some example benchmarks you can run.

With that, let's move on to some numbers

## Summary

Single node performance of wasmCloud given the constraints above was around 22-23k req/s. Clustered
performanace _per node_ (which should be multiplied by number of nodes) is around 17-18k req/s.

## Actual Benchmark Statistics

The benchmarks below were a 2 MacOS node cluster, each running the component and an HTTP server. As
you can see from the results, these two nodes handled 300 concurrent requests at a total throughput
of 35k req/s.

### Host 1 results

```
hey -z 20s -c 150 http://localhost:8080

Summary:
  Total:	20.0074 secs
  Slowest:	0.0666 secs
  Fastest:	0.0003 secs
  Average:	0.0085 secs
  Requests/sec:	17565.8222

  Total data:	5974599 bytes
  Size/request:	17 bytes

Response time histogram:
  0.000 [1]	|
  0.007 [155892]	|■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■
  0.014 [136008]	|■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■
  0.020 [50655]	|■■■■■■■■■■■■■
  0.027 [6317]	|■■
  0.033 [1368]	|
  0.040 [785]	|
  0.047 [333]	|
  0.053 [23]	|
  0.060 [14]	|
  0.067 [51]	|


Latency distribution:
  10% in 0.0020 secs
  25% in 0.0037 secs
  50% in 0.0084 secs
  75% in 0.0122 secs
  90% in 0.0152 secs
  95% in 0.0175 secs
  99% in 0.0247 secs

Details (average, fastest, slowest):
  DNS+dialup:	0.0000 secs, 0.0003 secs, 0.0666 secs
  DNS-lookup:	0.0000 secs, 0.0000 secs, 0.0134 secs
  req write:	0.0000 secs, 0.0000 secs, 0.0036 secs
  resp wait:	0.0085 secs, 0.0003 secs, 0.0665 secs
  resp read:	0.0000 secs, 0.0000 secs, 0.0092 secs

Status code distribution:
  [200]	351447 responses
```

### Host 2 results

```
hey -z 20s -c 150 http://localhost:8080/

Summary:
  Total:	20.0194 secs
  Slowest:	0.0772 secs
  Fastest:	0.0002 secs
  Average:	0.0079 secs
  Requests/sec:	18880.2130

  Total data:	6425116 bytes
  Size/request:	17 bytes

Response time histogram:
  0.000 [1]	|
  0.008 [191227]	|■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■
  0.016 [142722]	|■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■
  0.023 [37750]	|■■■■■■■■
  0.031 [4090]	|■
  0.039 [1604]	|
  0.046 [514]	|
  0.054 [19]	|
  0.062 [5]	|
  0.069 [15]	|
  0.077 [1]	|


Latency distribution:
  10% in 0.0009 secs
  25% in 0.0018 secs
  50% in 0.0073 secs
  75% in 0.0130 secs
  90% in 0.0161 secs
  95% in 0.0185 secs
  99% in 0.0263 secs

Details (average, fastest, slowest):
  DNS+dialup:	0.0000 secs, 0.0002 secs, 0.0772 secs
  DNS-lookup:	0.0000 secs, 0.0000 secs, 0.0031 secs
  req write:	0.0000 secs, 0.0000 secs, 0.0017 secs
  resp wait:	0.0079 secs, 0.0002 secs, 0.0771 secs
  resp read:	0.0000 secs, 0.0000 secs, 0.0055 secs

Status code distribution:
  [200]	377948 responses
```

## Planning for capacity

When observing the state of the system when running the tests, the CPUs were all fairly busy.
However, memory usage stayed fairly low. Testing with more real world applications will likely give
us a better idea of our memory footprint, but currently it seems that memory is not an issue when it
comes to raw throughput (which makes sense).

Based on other tests we ran (and the number of cores on the Mac processors), for high throughput you
likely need at least 8 cores per machine/VM/container, and possibly up to 16 cores. So if you take
the total throughput you need in req/s, then divide it by 17k, that should give you a _rough_ (and
we mean rough) estimate of the number of nodes needed to reach the amount of throughput desired.
