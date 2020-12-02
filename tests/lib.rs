mod common;
mod control;
mod generated;
mod no_lattice;
mod with_lattice;

use std::env::temp_dir;
use wasmcloud_host::Result;

#[cfg(test)]
#[ctor::ctor]
fn init() {
    println!("Purging provider cache");
    let path = temp_dir();
    let path = path.join("wasmcloudcache");
    let _ = ::std::fs::remove_dir_all(path);
}

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

#[actix_rt::test]
async fn distributed_echo() -> Result<()> {
    with_lattice::distributed_echo().await
}

#[actix_rt::test]
async fn link_on_third_host() -> Result<()> {
    with_lattice::link_on_third_host().await
}
/*
#[actix_rt::test]
async fn scaled_kvcounter() -> Result<()> {
    with_lattice::scaled_kvcounter().await
}
*/

#[actix_rt::test]
async fn control_basics() -> Result<()> {
    control::basics().await
}
