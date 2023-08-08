pub const ISSUER: &str = env!("ISSUER");

pub const RUST_HTTPSERVER: &str = concat!(env!("OUT_DIR"), "/rust-httpserver.par");
pub const RUST_HTTPSERVER_SUBJECT: &str = env!("RUST_HTTPSERVER_SUBJECT");

pub const RUST_KVREDIS: &str = concat!(env!("OUT_DIR"), "/rust-kvredis.par");
pub const RUST_KVREDIS_SUBJECT: &str = env!("RUST_KVREDIS_SUBJECT");

pub const RUST_NATS: &str = concat!(env!("OUT_DIR"), "/rust-nats.par");
pub const RUST_NATS_SUBJECT: &str = env!("RUST_NATS_SUBJECT");
