//! Runs `wasi:webgpu` commands inside wash-runtime for scripts/gpubench:
//! `run` is step 3 of the native gap bench, `turns` is the wasmCloud side of
//! the serverless bench. The guest's stdout and stderr are this process's.
//!
//! wash-runtime has no `wasi:webgpu` on Windows or s390x; there it only says so.

#[cfg(all(not(target_os = "windows"), not(target_arch = "s390x")))]
mod cli;
#[cfg(all(not(target_os = "windows"), not(target_arch = "s390x")))]
mod turns;
#[cfg(all(not(target_os = "windows"), not(target_arch = "s390x")))]
mod workload;

use std::process::ExitCode;

#[cfg(all(not(target_os = "windows"), not(target_arch = "s390x")))]
fn main() -> ExitCode {
    cli::main()
}

#[cfg(any(target_os = "windows", target_arch = "s390x"))]
fn main() -> ExitCode {
    eprintln!("bench-gpu: wash-runtime has no wasi:webgpu on this target");
    ExitCode::from(2)
}
