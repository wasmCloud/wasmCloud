pub const ISSUER: &str = env!("ISSUER");

pub const RUST_BLOBSTORE_FS: &str = concat!(env!("OUT_DIR"), "/rust-blobstore-fs.par");
pub const RUST_BLOBSTORE_FS_SUBJECT: &str = env!("RUST_BLOBSTORE_FS_SUBJECT");

pub const RUST_BLOBSTORE_S3: &str = concat!(env!("OUT_DIR"), "/rust-blobstore-s3.par");
pub const RUST_BLOBSTORE_S3_SUBJECT: &str = env!("RUST_BLOBSTORE_S3_SUBJECT");

// todo(vados-cosmonic): re-enable once wrpc_transport::{Encode,Receive}
// is implemented for either HashMap or Tuple(T1,T2)
// pub const RUST_HTTPCLIENT: &str = concat!(env!("OUT_DIR"), "/rust-httpclient.par");
// pub const RUST_HTTPCLIENT_SUBJECT: &str = env!("RUST_HTTPCLIENT_SUBJECT");

// todo(vados-cosmonic): re-enable once http-server is working
// pub const RUST_HTTPSERVER: &str = concat!(env!("OUT_DIR"), "/rust-httpserver.par");
// pub const RUST_HTTPSERVER_SUBJECT: &str = env!("RUST_HTTPSERVER_SUBJECT");

pub const RUST_KVREDIS: &str = concat!(env!("OUT_DIR"), "/rust-kvredis.par");
pub const RUST_KVREDIS_SUBJECT: &str = env!("RUST_KVREDIS_SUBJECT");

pub const RUST_KV_VAULT: &str = concat!(env!("OUT_DIR"), "/rust-kv-vault.par");
pub const RUST_KV_VAULT_SUBJECT: &str = env!("RUST_KV_VAULT_SUBJECT");

pub const RUST_NATS: &str = concat!(env!("OUT_DIR"), "/rust-nats.par");
pub const RUST_NATS_SUBJECT: &str = env!("RUST_NATS_SUBJECT");

pub const RUST_BLOBSTORE_S3_VAULT: &str = concat!(env!("OUT_DIR"), "/rust-blobstore-s3.par");
pub const RUST_BLOBSTORE_S3_VAULT_SUBJECT: &str = env!("RUST_BLOBSTORE_S3_SUBJECT");

pub const RUST_LATTICE_CONTROLLER: &str = concat!(env!("OUT_DIR"), "/rust-lattice-controller.par");
pub const RUST_LATTICE_CONTROLLER_SUBJECT: &str = env!("RUST_LATTICE_CONTROLLER_SUBJECT");
