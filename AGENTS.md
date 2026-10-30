# Repository instructions

- Do not write or run Python for repository work. Use Rust for new tooling and
  test harnesses, and existing shell tools for routine operations.
- Reuse the benchmark framework for benchmarks and pressure tests. Extend its
  shared records, renderers, runners, and reports instead of adding standalone
  drivers. Follow `crates/wash-runtime/benches/AGENTS.md` for runtime benches.
- Write terse code comments in simple American English. Describe the current
  contract or invariant, not history or what the code already says.
- Model each state machine as one enum behind a typed atomic newtype.
  Do not use parallel families of bare integer constants and atomics.
- Sign off every commit with a DCO `Signed-off-by` trailer (`git commit -s`).
