use super::Ctx;

use crate::capability::keyvalue::{readwrite, types, wasi_cloud_error};

use async_trait::async_trait;

#[async_trait]
impl readwrite::Host for Ctx {}

#[async_trait]
impl types::Host for Ctx {}

#[async_trait]
impl wasi_cloud_error::Host for Ctx {}
