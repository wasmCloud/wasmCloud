// use crate::wasmcloud::wadm::oam_types;
// use crate::wasmcloud::wadm::wadm_types;
// use wadm::model::{
//     CapabilityProperties, Component, ComponentProperties, ConfigProperty, LinkProperty, Manifest,
//     Metadata, Properties, Specification, Spread, SpreadScalerProperty, Trait, TraitProperty,
// };

// impl From<Manifest> for oam_types::OamManifest {
//     fn from(manifest: Manifest) -> Self {
//         oam_types::OamManifest {
//             api_version: manifest.api_version.to_string(),
//             kind: manifest.kind.to_string(),
//             metadata: manifest.metadata.into(),
//             spec: manifest.spec.into(),
//         }
//     }
// }

// impl From<Metadata> for oam_types::Metadata {
//     fn from(metadata: Metadata) -> Self {
//         oam_types::Metadata {
//             name: metadata.name,
//             annotations: metadata.annotations.into_iter().collect(),
//             labels: metadata.labels.into_iter().collect(),
//         }
//     }
// }

// impl From<Specification> for oam_types::Specification {
//     fn from(spec: Specification) -> Self {
//         oam_types::Specification {
//             components: spec.components.into_iter().map(|c| c.into()).collect(),
//         }
//     }
// }

// impl From<Component> for oam_types::Component {
//     fn from(component: Component) -> Self {
//         oam_types::Component {
//             name: component.name,
//             properties: component.properties.into(),
//             traits: component
//                 .traits
//                 .map(|traits| traits.into_iter().map(|t| t.into()).collect()),
//         }
//     }
// }

// impl From<Properties> for oam_types::Properties {
//     fn from(properties: Properties) -> Self {
//         match properties {
//             Properties::Component { properties } => {
//                 oam_types::Properties::Component(properties.into())
//             }
//             Properties::Capability { properties } => {
//                 oam_types::Properties::Capability(properties.into())
//             }
//         }
//     }
// }

// impl From<ComponentProperties> for oam_types::ComponentProperties {
//     fn from(properties: ComponentProperties) -> Self {
//         oam_types::ComponentProperties {
//             image: properties.image,
//             id: properties.id,
//             config: properties.config.into_iter().map(|c| c.into()).collect(),
//         }
//     }
// }

// impl From<CapabilityProperties> for oam_types::CapabilityProperties {
//     fn from(properties: CapabilityProperties) -> Self {
//         oam_types::CapabilityProperties {
//             image: properties.image,
//             id: properties.id,
//             config: properties.config.into_iter().map(|c| c.into()).collect(),
//         }
//     }
// }

// impl From<ConfigProperty> for oam_types::ConfigProperty {
//     fn from(property: ConfigProperty) -> Self {
//         oam_types::ConfigProperty {
//             name: property.name,
//             properties: property.properties.map(|props| props.into_iter().collect()),
//         }
//     }
// }

// impl From<Trait> for oam_types::Trait {
//     fn from(trait_: Trait) -> Self {
//         oam_types::Trait {
//             trait_type: trait_.trait_type,
//             properties: trait_.properties.into(),
//         }
//     }
// }

// impl From<TraitProperty> for oam_types::TraitProperty {
//     fn from(property: TraitProperty) -> Self {
//         match property {
//             TraitProperty::Link(link) => oam_types::TraitProperty::Link(link.into()),
//             TraitProperty::SpreadScaler(spread) => {
//                 oam_types::TraitProperty::Spreadscaler(spread.into())
//             }
//             TraitProperty::Custom(custom) => oam_types::TraitProperty::Custom(custom.to_string()),
//         }
//     }
// }

// impl From<LinkProperty> for oam_types::LinkProperty {
//     fn from(property: LinkProperty) -> Self {
//         oam_types::LinkProperty {
//             target: property.target,
//             namespace: property.namespace,
//             package: property.package,
//             interfaces: property.interfaces,
//             source_config: property
//                 .source_config
//                 .into_iter()
//                 .map(|c| c.into())
//                 .collect(),
//             target_config: property
//                 .target_config
//                 .into_iter()
//                 .map(|c| c.into())
//                 .collect(),
//             name: property.name,
//         }
//     }
// }

// impl From<SpreadScalerProperty> for oam_types::SpreadscalerProperty {
//     fn from(property: SpreadScalerProperty) -> Self {
//         oam_types::SpreadscalerProperty {
//             instances: property.instances as u32,
//             spread: property.spread.into_iter().map(|s| s.into()).collect(),
//         }
//     }
// }

// impl From<Spread> for oam_types::Spread {
//     fn from(spread: Spread) -> Self {
//         oam_types::Spread {
//             name: spread.name,
//             requirements: spread.requirements.into_iter().collect(),
//             weight: spread.weight.map(|w| w as u32),
//         }
//     }
// }

// impl From<wadm::server::ModelSummary> for wadm_types::ModelSummary {
//     fn from(summary: wadm::server::ModelSummary) -> Self {
//         wadm_types::ModelSummary {
//             name: summary.name,
//             version: summary.version,
//             description: summary.description,
//             deployed_version: summary.deployed_version,
//             status: summary.status.into(),
//             status_message: summary.status_message,
//         }
//     }
// }

// impl From<wadm::server::DeleteResult> for wadm_types::DeleteResult {
//     fn from(result: wadm::server::DeleteResult) -> Self {
//         match result {
//             wadm::server::DeleteResult::Deleted => wadm_types::DeleteResult::Deleted,
//             wadm::server::DeleteResult::Error => wadm_types::DeleteResult::Error,
//             wadm::server::DeleteResult::Noop => wadm_types::DeleteResult::Noop,
//         }
//     }
// }

// impl From<wadm::server::GetResult> for wadm_types::GetResult {
//     fn from(result: wadm::server::GetResult) -> Self {
//         match result {
//             wadm::server::GetResult::Error => wadm_types::GetResult::Error,
//             wadm::server::GetResult::Success => wadm_types::GetResult::Success,
//             wadm::server::GetResult::NotFound => wadm_types::GetResult::NotFound,
//         }
//     }
// }

// impl From<wadm::server::PutResult> for wadm_types::PutResult {
//     fn from(result: wadm::server::PutResult) -> Self {
//         match result {
//             wadm::server::PutResult::Error => wadm_types::PutResult::Error,
//             wadm::server::PutResult::Created => wadm_types::PutResult::Created,
//             wadm::server::PutResult::NewVersion => wadm_types::PutResult::NewVersion,
//         }
//     }
// }

// impl From<wadm::server::StatusType> for wadm_types::StatusType {
//     fn from(status: wadm::server::StatusType) -> Self {
//         match status {
//             wadm::server::StatusType::Undeployed => wadm_types::StatusType::Undeployed,
//             wadm::server::StatusType::Reconciling => wadm_types::StatusType::Reconciling,
//             wadm::server::StatusType::Deployed => wadm_types::StatusType::Deployed,
//             wadm::server::StatusType::Failed => wadm_types::StatusType::Failed,
//         }
//     }
// }

// impl From<wadm::server::DeleteModelResponse> for wadm_types::DeleteModelResponse {
//     fn from(response: wadm::server::DeleteModelResponse) -> Self {
//         wadm_types::DeleteModelResponse {
//             result: response.result.into(),
//             message: response.message,
//             undeploy: response.undeploy,
//         }
//     }
// }

// impl From<wadm_types::StatusType> for wadm::server::StatusType {
//     fn from(status: wadm_types::StatusType) -> Self {
//         match status {
//             wadm_types::StatusType::Undeployed => wadm::server::StatusType::Undeployed,
//             wadm_types::StatusType::Reconciling => wadm::server::StatusType::Reconciling,
//             wadm_types::StatusType::Deployed => wadm::server::StatusType::Deployed,
//             wadm_types::StatusType::Failed => wadm::server::StatusType::Failed,
//         }
//     }
// }

// impl From<wadm_types::StatusInfo> for wadm::server::StatusInfo {
//     fn from(info: wadm_types::StatusInfo) -> Self {
//         wadm::server::StatusInfo {
//             status_type: info.status_type.into(),
//             message: info.message,
//         }
//     }
// }

// impl From<wadm_types::ComponentStatus> for wadm::server::ComponentStatus {
//     fn from(status: wadm_types::ComponentStatus) -> Self {
//         wadm::server::ComponentStatus {
//             name: status.name,
//             component_type: status.component_type,
//             info: status.info.into(),
//             traits: status
//                 .traits
//                 .into_iter()
//                 .map(|t| wadm::server::TraitStatus {
//                     trait_type: t.trait_type,
//                     info: t.info.into(),
//                 })
//                 .collect(),
//         }
//     }
// }

// impl From<wadm_types::TraitStatus> for wadm::server::TraitStatus {
//     fn from(status: wadm_types::TraitStatus) -> Self {
//         wadm::server::TraitStatus {
//             trait_type: status.trait_type,
//             info: status.info.into(),
//         }
//     }
// }

// impl From<wadm_types::StatusResult> for wadm::server::StatusResult {
//     fn from(result: wadm_types::StatusResult) -> Self {
//         match result {
//             wadm_types::StatusResult::Error => wadm::server::StatusResult::Error,
//             wadm_types::StatusResult::Ok => wadm::server::StatusResult::Ok,
//             wadm_types::StatusResult::NotFound => wadm::server::StatusResult::NotFound,
//         }
//     }
// }

// impl From<wadm_types::DeployResponse> for wadm::server::DeployModelResponse {
//     fn from(response: wadm_types::DeployResponse) -> Self {
//         wadm::server::DeployModelResponse {
//             result: match response.result {
//                 wadm_types::DeployResult::Error => wadm::server::DeployResult::Error,
//                 wadm_types::DeployResult::Acknowledged => wadm::server::DeployResult::Acknowledged,
//                 wadm_types::DeployResult::NotFound => wadm::server::DeployResult::NotFound,
//             },
//             message: response.message,
//         }
//     }
// }
// impl From<oam_types::OamManifest> for Manifest {
//     fn from(manifest: oam_types::OamManifest) -> Self {
//         Manifest {
//             api_version: manifest.api_version,
//             kind: manifest.kind,
//             metadata: manifest.metadata.into(),
//             spec: manifest.spec.into(),
//         }
//     }
// }

// impl From<oam_types::Metadata> for Metadata {
//     fn from(metadata: oam_types::Metadata) -> Self {
//         Metadata {
//             name: metadata.name,
//             annotations: metadata.annotations.into_iter().collect(),
//             labels: metadata.labels.into_iter().collect(),
//         }
//     }
// }

// impl From<oam_types::Specification> for Specification {
//     fn from(spec: oam_types::Specification) -> Self {
//         Specification {
//             components: spec.components.into_iter().map(|c| c.into()).collect(),
//         }
//     }
// }

// impl From<oam_types::Component> for Component {
//     fn from(component: oam_types::Component) -> Self {
//         Component {
//             name: component.name,
//             properties: component.properties.into(),
//             traits: component
//                 .traits
//                 .map(|traits| traits.into_iter().map(|t| t.into()).collect()),
//         }
//     }
// }

// impl From<oam_types::Properties> for Properties {
//     fn from(properties: oam_types::Properties) -> Self {
//         match properties {
//             oam_types::Properties::Component(properties) => Properties::Component {
//                 properties: properties.into(),
//             },
//             oam_types::Properties::Capability(properties) => Properties::Capability {
//                 properties: properties.into(),
//             },
//         }
//     }
// }

// impl From<oam_types::ComponentProperties> for ComponentProperties {
//     fn from(properties: oam_types::ComponentProperties) -> Self {
//         ComponentProperties {
//             image: properties.image,
//             id: properties.id,
//             config: properties.config.into_iter().map(|c| c.into()).collect(),
//         }
//     }
// }

// impl From<oam_types::CapabilityProperties> for CapabilityProperties {
//     fn from(properties: oam_types::CapabilityProperties) -> Self {
//         CapabilityProperties {
//             image: properties.image,
//             id: properties.id,
//             config: properties.config.into_iter().map(|c| c.into()).collect(),
//         }
//     }
// }

// impl From<oam_types::ConfigProperty> for ConfigProperty {
//     fn from(property: oam_types::ConfigProperty) -> Self {
//         ConfigProperty {
//             name: property.name,
//             properties: property.properties.map(|props| props.into_iter().collect()),
//         }
//     }
// }

// impl From<oam_types::Trait> for Trait {
//     fn from(trait_: oam_types::Trait) -> Self {
//         Trait {
//             trait_type: trait_.trait_type,
//             properties: trait_.properties.into(),
//         }
//     }
// }

// impl From<oam_types::TraitProperty> for TraitProperty {
//     fn from(property: oam_types::TraitProperty) -> Self {
//         match property {
//             oam_types::TraitProperty::Link(link) => TraitProperty::Link(link.into()),
//             oam_types::TraitProperty::Spreadscaler(spread) => {
//                 TraitProperty::SpreadScaler(spread.into())
//             }
//             oam_types::TraitProperty::Custom(custom) => {
//                 TraitProperty::Custom(serde_json::value::Value::String(custom))
//             }
//         }
//     }
// }

// impl From<oam_types::LinkProperty> for LinkProperty {
//     fn from(property: oam_types::LinkProperty) -> Self {
//         LinkProperty {
//             target: property.target,
//             namespace: property.namespace,
//             package: property.package,
//             interfaces: property.interfaces,
//             source_config: property
//                 .source_config
//                 .into_iter()
//                 .map(|c| c.into())
//                 .collect(),
//             target_config: property
//                 .target_config
//                 .into_iter()
//                 .map(|c| c.into())
//                 .collect(),
//             name: property.name,
//         }
//     }
// }

// impl From<oam_types::SpreadscalerProperty> for SpreadScalerProperty {
//     fn from(property: oam_types::SpreadscalerProperty) -> Self {
//         SpreadScalerProperty {
//             instances: property.instances as usize,
//             spread: property.spread.into_iter().map(|s| s.into()).collect(),
//         }
//     }
// }

// impl From<oam_types::Spread> for wadm::model::Spread {
//     fn from(spread: oam_types::Spread) -> Self {
//         wadm::model::Spread {
//             name: spread.name,
//             requirements: spread.requirements.into_iter().collect(),
//             weight: spread.weight.map(|w| w as usize),
//         }
//     }
// }

// impl From<wadm::server::DeployModelResponse> for wadm_types::DeployResponse {
//     fn from(response: wadm::server::DeployModelResponse) -> Self {
//         wadm_types::DeployResponse {
//             result: match response.result {
//                 wadm::server::DeployResult::Error => wadm_types::DeployResult::Error,
//                 wadm::server::DeployResult::Acknowledged => wadm_types::DeployResult::Acknowledged,
//                 wadm::server::DeployResult::NotFound => wadm_types::DeployResult::NotFound,
//             },
//             message: response.message,
//         }
//     }
// }

// impl From<wadm::server::PutModelResponse> for wadm_types::PutModelResponse {
//     fn from(response: wadm::server::PutModelResponse) -> Self {
//         wadm_types::PutModelResponse {
//             result: response.result.into(),
//             total_versions: response.total_versions as u32,
//             current_version: response.current_version,
//             message: response.message,
//             name: response.name,
//         }
//     }
// }

// impl From<wadm::server::VersionResponse> for wadm_types::VersionResponse {
//     fn from(response: wadm::server::VersionResponse) -> Self {
//         wadm_types::VersionResponse {
//             result: response.result.into(),
//             message: response.message,
//             versions: response
//                 .versions
//                 .into_iter()
//                 .map(|v| wadm_types::VersionInfo {
//                     version: v.version,
//                     deployed: v.deployed,
//                 })
//                 .collect(),
//         }
//     }
// }

// impl From<wadm::server::StatusResponse> for wadm_types::StatusResponse {
//     fn from(response: wadm::server::StatusResponse) -> Self {
//         wadm_types::StatusResponse {
//             result: response.result.into(),
//             message: response.message,
//             status: response.status.map(|s| wadm_types::Status {
//                 version: s.version,
//                 info: s.info.into(),
//                 components: s
//                     .components
//                     .into_iter()
//                     .map(|c| wadm_types::ComponentStatus {
//                         name: c.name,
//                         component_type: c.component_type,
//                         info: c.info.into(),
//                         traits: c
//                             .traits
//                             .into_iter()
//                             .map(|t| wadm_types::TraitStatus {
//                                 trait_type: t.trait_type,
//                                 info: t.info.into(),
//                             })
//                             .collect(),
//                     })
//                     .collect(),
//             }),
//         }
//     }
// }

// impl From<wadm::server::GetModelResponse> for wadm_types::GetModelResponse {
//     fn from(response: wadm::server::GetModelResponse) -> Self {
//         wadm_types::GetModelResponse {
//             result: response.result.into(),
//             message: response.message,
//             manifest: response.manifest.map(|m| m.into()),
//         }
//     }
// }

// impl From<wadm::server::StatusResult> for wadm_types::StatusResult {
//     fn from(result: wadm::server::StatusResult) -> Self {
//         match result {
//             wadm::server::StatusResult::Error => wadm_types::StatusResult::Error,
//             wadm::server::StatusResult::Ok => wadm_types::StatusResult::Ok,
//             wadm::server::StatusResult::NotFound => wadm_types::StatusResult::NotFound,
//         }
//     }
// }

// impl From<wadm::server::StatusInfo> for wadm_types::StatusInfo {
//     fn from(info: wadm::server::StatusInfo) -> Self {
//         wadm_types::StatusInfo {
//             status_type: info.status_type.into(),
//             message: info.message,
//         }
//     }
// }
