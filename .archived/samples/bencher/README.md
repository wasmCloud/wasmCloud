# Bencher

Runs a tight loop of 10,000 iterations on an actor that makes no host capability requests. The benchmark actor performs
a de-serialization of the HTTP request, and a de-serialization of the body, and then performs a serialization of the HTTP
response after performing a few match calculations. This simulates (roughly) the kind of work being done by actors without
taking a latency loss for capability use.

You will need to `make release` in the bench actor's directory. You will also want to run the `release` version o this binary or use `cargo run --release`. This makes a _huge_ difference (a factor of 100, usually) in the performance of the benchmark test.
