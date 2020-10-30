mod common;
mod generated;
mod no_lattice;

use std::error::Error;
use wascc_host::{HostBuilder, Result};

#[actix_rt::test]
async fn start_and_execute_echo() -> Result<()> {
    no_lattice::start_and_execute_echo().await
}

#[actix_rt::test]
async fn kvcounter_basic() -> Result<()> {
    no_lattice::kvcounter_basic().await
}

#[actix_rt::test]
async fn kvcounter_binding_first() -> Result<()> {
    no_lattice::kvcounter_binding_first().await
}

#[actix_rt::test]
async fn kvcounter_start_stop() -> Result<()> {
    no_lattice::kvcounter_start_stop().await
}
