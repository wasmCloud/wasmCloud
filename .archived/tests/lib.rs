#[cfg(test)]
mod common;
mod no_lattice;
mod with_lattice;

use std::env::temp_dir;

#[cfg(test)]
#[ctor::ctor]
fn init() {
    println!("Purging provider cache");
    let path = temp_dir();
    let path = path.join("wasmcloudcache");
    let _ = ::std::fs::remove_dir_all(path);
}

#[actix_rt::test]
async fn unlink_provider() {
    let res = no_lattice::unlink_provider().await;
    if let Err(ref e) = res {
        println!("{}", e);
    }
    assert!(res.is_ok());
}

#[actix_rt::test]
async fn distributed_unlink_provider() {
    let res = with_lattice::distributed_unlink().await;
    if let Err(ref e) = res {
        println!("{}", e);
    }
    assert!(res.is_ok());
}

#[actix_rt::test]
async fn actor_to_actor_call_alias() {
    let res = no_lattice::actor_to_actor_call_alias().await;
    if let Err(ref e) = res {
        println!("{}", e);
    }
    assert!(res.is_ok());
}

#[actix_rt::test]
async fn kvcounter_basic() {
    let res = no_lattice::kvcounter_basic().await;
    if let Err(ref e) = res {
        println!("{}", e);
    }
    assert!(res.is_ok());
}

#[actix_rt::test]
async fn kvcounter_start_stop() {
    let res = no_lattice::kvcounter_start_stop().await;
    if let Err(ref e) = res {
        println!("{}", e);
    }
    assert!(res.is_ok());
}

#[actix_rt::test]
async fn kvcounter_link_first() {
    let res = no_lattice::kvcounter_link_first().await;
    if let Err(ref e) = res {
        println!("{}", e);
    }
    assert!(res.is_ok());
}

#[actix_rt::test]
async fn link_on_third_host() {
    let res = with_lattice::link_on_third_host().await;
    if let Err(ref e) = res {
        println!("{}", e);
    }
    assert!(res.is_ok());
}

#[actix_rt::test]
async fn redis_kvcache() {
    let res = with_lattice::redis_kvcache().await;
    if let Err(ref e) = res {
        println!("{}", e);
    }
    assert!(res.is_ok());
}

#[actix_rt::test]
async fn extras_provider() {
    let res = no_lattice::extras_provider().await;
    if let Err(ref e) = res {
        println!("{}", e);
    }
    assert!(res.is_ok());
}
