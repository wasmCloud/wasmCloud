//! Data types used when interacting with the control interface of a wasmCloud lattice

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Result;

/// A control interface response that wraps a response payload, a success flag, and a message
/// with additional context if necessary.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[non_exhaustive]
pub struct CtlResponse<T> {
    /// Whether the request succeeded
    pub(crate) success: bool,
    /// A message with additional context about the response
    pub(crate) message: String,
    /// The response data, if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) response: Option<T>,
}

impl<T> CtlResponse<T> {
    /// Create a [`CtlResponse`] with provided response data
    #[must_use]
    pub fn ok(response: T) -> Self {
        CtlResponse {
            success: true,
            message: String::new(),
            response: Some(response),
        }
    }

    /// Get whether the request succeeded
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.success
    }

    /// Get the message included in the response
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Get the internal data of the response (if any)
    #[must_use]
    pub fn data(&self) -> Option<&T> {
        self.response.as_ref()
    }

    /// Take the internal data
    #[must_use]
    pub fn into_data(self) -> Option<T> {
        self.response
    }
}

impl CtlResponse<()> {
    /// Helper function to return a successful response without
    /// a message or a payload.
    #[must_use]
    pub fn success(message: String) -> Self {
        CtlResponse {
            success: true,
            message,
            response: None,
        }
    }

    /// Helper function to return an unsuccessful response with
    /// a message but no payload. Note that this implicitly is
    /// typing the inner payload as `()` for efficiency.
    #[must_use]
    pub fn error(message: &str) -> Self {
        CtlResponse {
            success: false,
            message: message.to_string(),
            response: None,
        }
    }
}

/// Command a host to scale a component
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ScaleComponentCommand {
    /// Image reference for the component.
    #[serde(default)]
    pub(crate) component_ref: String,
    /// Unique identifier of the component to scale.
    pub(crate) component_id: String,
    /// Optional set of annotations used to describe the nature of this component scale command. For
    /// example, autonomous agents may wish to "tag" scale requests as part of a given deployment
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) annotations: Option<BTreeMap<String, String>>,
    /// The maximum number of concurrent executing instances of this component. Setting this to `0` will
    /// stop the component.
    // NOTE: renaming to `count` lets us remain backwards compatible for a few minor versions
    #[serde(default, alias = "count", rename = "count")]
    pub(crate) max_instances: u32,
    /// Host ID on which to scale this component
    #[serde(default)]
    pub(crate) host_id: String,
    /// A list of named configs to use for this component. It is not required to specify a config.
    /// Configs are merged together before being given to the component, with values from the right-most
    /// config in the list taking precedence. For example, given ordered configs foo {a: 1, b: 2},
    /// bar {b: 3, c: 4}, and baz {c: 5, d: 6}, the resulting config will be: {a: 1, b: 3, c: 5, d:
    /// 6}
    #[serde(default)]
    pub(crate) config: Vec<String>,
    #[serde(default)]
    /// Whether to perform an update if the details of the component (ex. component ID) change as
    /// part of the scale request.
    ///
    /// Normally this is implemented by the receiver (ex. wasmcloud host) as a *separate* update component call
    /// being made shortly after this command (scale) is processed.
    pub(crate) allow_update: bool,
}

impl ScaleComponentCommand {
    #[must_use]
    pub fn component_ref(&self) -> &str {
        &self.component_ref
    }

    #[must_use]
    pub fn component_id(&self) -> &str {
        &self.component_id
    }

    #[must_use]
    pub fn allow_update(&self) -> bool {
        self.allow_update
    }

    #[must_use]
    pub fn config(&self) -> &Vec<String> {
        &self.config
    }

    #[must_use]
    pub fn annotations(&self) -> Option<&BTreeMap<String, String>> {
        self.annotations.as_ref()
    }

    #[must_use]
    pub fn max_instances(&self) -> u32 {
        self.max_instances
    }

    #[must_use]
    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    #[must_use]
    pub fn builder() -> ScaleComponentCommandBuilder {
        ScaleComponentCommandBuilder::default()
    }
}

/// Builder that produces [`ScaleComponentCommand`]s
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct ScaleComponentCommandBuilder {
    component_ref: Option<String>,
    component_id: Option<String>,
    annotations: Option<BTreeMap<String, String>>,
    max_instances: Option<u32>,
    host_id: Option<String>,
    config: Option<Vec<String>>,
    allow_update: Option<bool>,
}

impl ScaleComponentCommandBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn component_ref(mut self, v: &str) -> Self {
        self.component_ref = Some(v.into());
        self
    }

    #[must_use]
    pub fn component_id(mut self, v: &str) -> Self {
        self.component_id = Some(v.into());
        self
    }

    #[must_use]
    pub fn annotations(mut self, v: impl Into<BTreeMap<String, String>>) -> Self {
        self.annotations = Some(v.into());
        self
    }

    #[must_use]
    pub fn max_instances(mut self, v: u32) -> Self {
        self.max_instances = Some(v);
        self
    }

    #[must_use]
    pub fn host_id(mut self, v: &str) -> Self {
        self.host_id = Some(v.into());
        self
    }

    #[must_use]
    pub fn config(mut self, v: Vec<String>) -> Self {
        self.config = Some(v);
        self
    }

    #[must_use]
    pub fn allow_update(mut self, v: bool) -> Self {
        self.allow_update = Some(v);
        self
    }

    pub fn build(self) -> Result<ScaleComponentCommand> {
        Ok(ScaleComponentCommand {
            component_ref: self
                .component_ref
                .ok_or_else(|| "component ref is required for scaling components".to_string())?,
            component_id: self
                .component_id
                .ok_or_else(|| "component id is required for scaling components".to_string())?,
            annotations: self.annotations,
            max_instances: self.max_instances.unwrap_or(0),
            host_id: self
                .host_id
                .ok_or_else(|| "host id is required for scaling hosts host".to_string())?,
            config: self.config.unwrap_or_default(),
            allow_update: self.allow_update.unwrap_or_default(),
        })
    }
}

/// A command sent to a host requesting a capability provider be started with the
/// given link name and optional configuration.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct StartProviderCommand {
    /// Unique identifier of the provider to start.
    provider_id: String,
    /// The image reference of the provider to be started
    #[serde(default)]
    provider_ref: String,
    /// The host ID on which to start the provider
    #[serde(default)]
    host_id: String,
    /// A list of named configs to use for this provider. It is not required to specify a config.
    /// Configs are merged together before being given to the provider, with values from the right-most
    /// config in the list taking precedence. For example, given ordered configs foo {a: 1, b: 2},
    /// bar {b: 3, c: 4}, and baz {c: 5, d: 6}, the resulting config will be: {a: 1, b: 3, c: 5, d:
    /// 6}
    #[serde(default)]
    config: Vec<String>,
    /// Optional set of annotations used to describe the nature of this provider start command. For
    /// example, autonomous agents may wish to "tag" start requests as part of a given deployment
    #[serde(default, skip_serializing_if = "Option::is_none")]
    annotations: Option<BTreeMap<String, String>>,
}

impl StartProviderCommand {
    #[must_use]
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    #[must_use]
    pub fn provider_ref(&self) -> &str {
        &self.provider_ref
    }

    #[must_use]
    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    #[must_use]
    pub fn config(&self) -> &Vec<String> {
        &self.config
    }

    #[must_use]
    pub fn annotations(&self) -> Option<&BTreeMap<String, String>> {
        self.annotations.as_ref()
    }

    #[must_use]
    pub fn builder() -> StartProviderCommandBuilder {
        StartProviderCommandBuilder::default()
    }
}

/// A builder that produces [`StartProviderCommand`]s
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct StartProviderCommandBuilder {
    host_id: Option<String>,
    provider_id: Option<String>,
    provider_ref: Option<String>,
    annotations: Option<BTreeMap<String, String>>,
    config: Option<Vec<String>>,
}

impl StartProviderCommandBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn provider_ref(mut self, v: &str) -> Self {
        self.provider_ref = Some(v.into());
        self
    }

    #[must_use]
    pub fn provider_id(mut self, v: &str) -> Self {
        self.provider_id = Some(v.into());
        self
    }

    #[must_use]
    pub fn annotations(mut self, v: impl Into<BTreeMap<String, String>>) -> Self {
        self.annotations = Some(v.into());
        self
    }

    #[must_use]
    pub fn host_id(mut self, v: &str) -> Self {
        self.host_id = Some(v.into());
        self
    }

    #[must_use]
    pub fn config(mut self, v: Vec<String>) -> Self {
        self.config = Some(v);
        self
    }

    pub fn build(self) -> Result<StartProviderCommand> {
        Ok(StartProviderCommand {
            provider_ref: self
                .provider_ref
                .ok_or_else(|| "provider ref is required for starting providers".to_string())?,
            provider_id: self
                .provider_id
                .ok_or_else(|| "provider id is required for starting providers".to_string())?,
            annotations: self.annotations,
            host_id: self
                .host_id
                .ok_or_else(|| "host id is required for starting providers".to_string())?,
            config: self.config.unwrap_or_default(),
        })
    }
}

/// A command sent to request that the given host purge and stop
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct StopHostCommand {
    /// The ID of the target host
    #[serde(default)]
    pub(crate) host_id: String,
    /// An optional timeout, in seconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timeout: Option<u64>,
}

impl StopHostCommand {
    #[must_use]
    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    #[must_use]
    pub fn timeout(&self) -> Option<u64> {
        self.timeout
    }

    #[must_use]
    pub fn builder() -> StopHostCommandBuilder {
        StopHostCommandBuilder::default()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct StopHostCommandBuilder {
    host_id: Option<String>,
    timeout: Option<u64>,
}

impl StopHostCommandBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn host_id(mut self, v: &str) -> Self {
        self.host_id = Some(v.into());
        self
    }

    #[must_use]
    pub fn timeout(mut self, v: u64) -> Self {
        self.timeout = Some(v);
        self
    }

    pub fn build(self) -> Result<StopHostCommand> {
        Ok(StopHostCommand {
            host_id: self
                .host_id
                .ok_or_else(|| "host id is required for stopping host".to_string())?,
            timeout: self.timeout,
        })
    }
}

/// A request to stop the given provider on the indicated host
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct StopProviderCommand {
    /// Host ID on which to stop the provider
    #[serde(default)]
    pub(crate) host_id: String,
    /// Unique identifier for the provider to stop.
    #[serde(default, alias = "provider_ref")]
    pub(crate) provider_id: String,
}

impl StopProviderCommand {
    #[must_use]
    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    #[must_use]
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    #[must_use]
    pub fn builder() -> StopProviderCommandBuilder {
        StopProviderCommandBuilder::default()
    }
}

/// Builder for [`StopProviderCommand`]s
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct StopProviderCommandBuilder {
    host_id: Option<String>,
    provider_id: Option<String>,
}

impl StopProviderCommandBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn host_id(mut self, v: &str) -> Self {
        self.host_id = Some(v.into());
        self
    }

    #[must_use]
    pub fn provider_id(mut self, v: &str) -> Self {
        self.provider_id = Some(v.into());
        self
    }

    pub fn build(self) -> Result<StopProviderCommand> {
        Ok(StopProviderCommand {
            host_id: self
                .host_id
                .ok_or_else(|| "host id is required for stopping provider".to_string())?,
            provider_id: self
                .provider_id
                .ok_or_else(|| "provider id is required for stopping provider".to_string())?,
        })
    }
}

/// A command instructing a specific host to perform a live update
/// on the indicated component by supplying a new image reference. Note that
/// live updates are only possible through image references
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct UpdateComponentCommand {
    /// The component's 56-character unique ID
    #[serde(default)]
    pub(crate) component_id: String,
    /// Optional set of annotations used to describe the nature of this
    /// update request. Only component instances that have matching annotations
    /// will be upgraded, allowing for instance isolation by
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) annotations: Option<BTreeMap<String, String>>,
    /// The host ID of the host to perform the live update
    #[serde(default)]
    pub(crate) host_id: String,
    /// The new image reference of the upgraded version of this component
    #[serde(default)]
    pub(crate) new_component_ref: String,
}

impl UpdateComponentCommand {
    #[must_use]
    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    #[must_use]
    pub fn component_id(&self) -> &str {
        &self.component_id
    }

    #[must_use]
    pub fn new_component_ref(&self) -> &str {
        &self.new_component_ref
    }

    #[must_use]
    pub fn annotations(&self) -> Option<&BTreeMap<String, String>> {
        self.annotations.as_ref()
    }

    #[must_use]
    pub fn builder() -> UpdateComponentCommandBuilder {
        UpdateComponentCommandBuilder::default()
    }
}

/// Builder for [`UpdateComponentCommand`]s
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct UpdateComponentCommandBuilder {
    host_id: Option<String>,
    component_id: Option<String>,
    new_component_ref: Option<String>,
    annotations: Option<BTreeMap<String, String>>,
}

impl UpdateComponentCommandBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn host_id(mut self, v: &str) -> Self {
        self.host_id = Some(v.into());
        self
    }

    #[must_use]
    pub fn component_id(mut self, v: &str) -> Self {
        self.component_id = Some(v.into());
        self
    }

    #[must_use]
    pub fn new_component_ref(mut self, v: &str) -> Self {
        self.new_component_ref = Some(v.into());
        self
    }

    #[must_use]
    pub fn annotations(mut self, v: impl Into<BTreeMap<String, String>>) -> Self {
        self.annotations = Some(v.into());
        self
    }

    pub fn build(self) -> Result<UpdateComponentCommand> {
        Ok(UpdateComponentCommand {
            host_id: self
                .host_id
                .ok_or_else(|| "host id is required for updating components".to_string())?,
            component_id: self
                .component_id
                .ok_or_else(|| "component id is required for updating components".to_string())?,
            new_component_ref: self.new_component_ref.ok_or_else(|| {
                "new component ref is required for updating components".to_string()
            })?,
            annotations: self.annotations,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        ScaleComponentCommand, StartProviderCommand, StopHostCommand, StopProviderCommand,
        UpdateComponentCommand,
    };

    #[test]
    fn scale_component_command_builder() {
        assert_eq!(
            ScaleComponentCommand {
                component_ref: "component_ref".into(),
                component_id: "component_id".into(),
                host_id: "host_id".into(),
                config: vec!["c".into()],
                allow_update: true,
                annotations: Some(BTreeMap::from([("a".into(), "b".into())])),
                max_instances: 1,
            },
            ScaleComponentCommand::builder()
                .component_ref("component_ref")
                .component_id("component_id")
                .host_id("host_id")
                .config(vec!["c".into()])
                .allow_update(true)
                .annotations(BTreeMap::from([("a".into(), "b".into())]))
                .max_instances(1)
                .build()
                .unwrap()
        )
    }

    #[test]
    fn start_provider_command_builder() {
        assert_eq!(
            StartProviderCommand {
                provider_id: "provider_id".into(),
                provider_ref: "provider_ref".into(),
                host_id: "host_id".into(),
                config: vec!["p".into()],
                annotations: Some(BTreeMap::from([("a".into(), "b".into())])),
            },
            StartProviderCommand::builder()
                .provider_id("provider_id")
                .provider_ref("provider_ref")
                .host_id("host_id")
                .config(vec!["p".into()])
                .annotations(BTreeMap::from([("a".into(), "b".into())]))
                .build()
                .unwrap()
        )
    }

    #[test]
    fn stop_host_command_builder() {
        assert_eq!(
            StopHostCommand {
                host_id: "host_id".into(),
                timeout: Some(1),
            },
            StopHostCommand::builder()
                .host_id("host_id")
                .timeout(1)
                .build()
                .unwrap()
        )
    }

    #[test]
    fn stop_provider_command_builder() {
        assert_eq!(
            StopProviderCommand {
                host_id: "host_id".into(),
                provider_id: "provider_id".into(),
            },
            StopProviderCommand::builder()
                .provider_id("provider_id")
                .host_id("host_id")
                .build()
                .unwrap()
        )
    }

    #[test]
    fn update_component_command_builder() {
        assert_eq!(
            UpdateComponentCommand {
                host_id: "host_id".into(),
                component_id: "component_id".into(),
                new_component_ref: "new_component_ref".into(),
                annotations: Some(BTreeMap::from([("a".into(), "b".into())])),
            },
            UpdateComponentCommand::builder()
                .host_id("host_id")
                .component_id("component_id")
                .new_component_ref("new_component_ref")
                .annotations(BTreeMap::from([("a".into(), "b".into())]))
                .build()
                .unwrap()
        )
    }
}
