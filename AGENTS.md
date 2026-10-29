# Project conventions

- Write code comments in terse, simple American English. Describe the current contract or invariant, not history. Remove comments that restate the code.
- Model each state machine as one enum behind a typed atomic newtype. Do not use parallel families of bare `u8` constants read directly from `AtomicU8`.
- Sign off every commit with a DCO Signed-off-by trailer (`git commit -s`).
- Write standalone GitHub Actions scripts as Node.js ES modules (`.mjs`) in `.github/scripts/`, not shell (`.sh`) scripts. Keep workflow inline shell steps short; move substantial logic into `.mjs` scripts. This convention applies to CI scripts, not shell scripts elsewhere in the project.
