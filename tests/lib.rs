mod common;
mod generated;
mod no_lattice;

use std::error::Error;
use wascc_host::HostBuilder;

#[actix_rt::test]
async fn start_and_execute_echo() -> Result<(), Box<dyn Error + Sync + Send>> {
    no_lattice::start_and_execute_echo().await
}

#[actix_rt::test]
async fn kvcounter_basic() -> Result<(), Box<dyn Error + Sync + Send>> {
    no_lattice::kvcounter_basic().await
}
