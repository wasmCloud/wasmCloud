# Benchmark Actor

This actor is designed to be used to do performance calculations. It performs de-serialization and serialization
of an internal payload, does a couple of math operations, and does not make use of any other capability providers,
allowing it to be invoked directly via `call_actor`, via the lattice, or via the HTTP provider, depending on which
round-trip time you're looking to test.
