use anyhow::{anyhow, bail, Result};
use cloudevents::event::{AttributesReader, Event};
use tokio::sync::mpsc::Receiver;
use tokio::time::{Duration, Instant};

use crate::lib::component::ComponentScaledInfo;

/// Useful parts of a `CloudEvent` coming in from the wasmbus.
#[derive(Debug, Clone)]
struct CloudEventData {
    event_type: String,
    source: String,
    data: serde_json::Value,
}

/// Small helper to easily get a String value out of a JSON object.
fn get_string_data_from_json(json: &serde_json::Value, key: &str) -> Result<String> {
    Ok(json
        .get(key)
        .ok_or_else(|| anyhow!("No {} key found in json data", key))?
        .as_str()
        .ok_or_else(|| anyhow!("{} is not a string", key))?
        .to_string())
}

/// Get the useful parts out of a wasmbus cloud event.
fn get_wasmbus_event_info(event: Event) -> Result<CloudEventData> {
    let data: serde_json::Value = event
        .data()
        .ok_or_else(|| anyhow!("No data in event"))?
        .clone()
        .try_into()?;

    Ok(CloudEventData {
        event_type: event.ty().to_string(),
        source: event.source().to_string(),
        data,
    })
}

/// The potential outcomes of an event that has been found.
///
/// It can either succeed or fail. This enum should only be returned if we found the applicable event.
/// If we did not find the event or another error occurred, use the `Err` variant of a `Result` wrapping around this enum.
pub enum FindEventOutcome<T> {
    Success(T),
    Failure(anyhow::Error),
}

/// The potential outcomes of a function check on an event.
/// Because we can pass events that are not applicable to the event we are looking for, we need the `NotApplicable` variant to skip these events.
pub enum EventCheckOutcome<T> {
    Success(T),
    Failure(anyhow::Error),
    NotApplicable,
}

/// Uses the NATS receiver to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
///
/// Takes a `check_function`, which receives each event coming in from the receiver. This function must return a `Result<EventCheckOutcome>`.
///
/// If the applicable response event is found (either started or failed to start), the `Ok` variant of the `Result` will be returned,
/// with the `FindEventOutcome` enum containing the success or failure state of the event.
///
/// If the timeout is reached or another error occurs, the `Err` variant of the `Result` will be returned.
///
/// You can use the generics in `EventCheckOutcome` and `FindEventOutcome` to return any data from the event out of your `check_function`.
async fn find_event<T>(
    receiver: &mut Receiver<Event>,
    timeout: Duration,
    check_function: impl Fn(Event) -> Result<EventCheckOutcome<T>>,
) -> Result<FindEventOutcome<T>> {
    let start = Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            bail!("Timeout waiting for event");
        }

        match tokio::time::timeout(timeout - elapsed, receiver.recv()).await {
            Ok(Some(event)) => {
                let outcome = check_function(event)?;

                match outcome {
                    EventCheckOutcome::Success(success_data) => {
                        return Ok(FindEventOutcome::Success(success_data))
                    }
                    EventCheckOutcome::Failure(e) => return Ok(FindEventOutcome::Failure(e)),
                    EventCheckOutcome::NotApplicable => continue,
                }
            }
            Err(_e) => {
                return Ok(FindEventOutcome::Failure(anyhow!(
                    "Timed out waiting for applicable event, operation may have failed"
                )))
            }
            // Should only happen due to an internal failure with the events receiver
            Ok(None) => {
                return Ok(FindEventOutcome::Failure(anyhow!(
                    "Channel dropped before event was received, please report this at https://github.com/wasmCloud/wasmCloud/issues with details to reproduce"
                )))
            }

        }
    }
}

/// Uses the NATS receiver to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
///
/// If the applicable component start response event is found (either started or failed to start), the `Ok` variant of the `Result` will be returned,
/// with the `FindEventOutcome` enum containing the success or failure state of the event.
///
/// If the timeout is reached or another error occurs, the `Err` variant of the `Result` will be returned.
pub async fn wait_for_component_scaled_event(
    receiver: &mut Receiver<Event>,
    timeout: Duration,
    host_id: impl AsRef<str>,
    component_ref: impl AsRef<str>,
) -> Result<FindEventOutcome<ComponentScaledInfo>> {
    let host_id = host_id.as_ref();
    let component_ref = component_ref.as_ref();
    let check_function = move |event: Event| {
        let cloud_event = get_wasmbus_event_info(event)?;

        if cloud_event.source != host_id {
            return Ok(EventCheckOutcome::NotApplicable);
        }

        match cloud_event.event_type.as_str() {
            "com.wasmcloud.lattice.component_scaled" => {
                let image_ref = get_string_data_from_json(&cloud_event.data, "image_ref")?;

                if image_ref == component_ref {
                    let component_id =
                        get_string_data_from_json(&cloud_event.data, "component_id")?;
                    return Ok(EventCheckOutcome::Success(ComponentScaledInfo {
                        host_id: host_id.into(),
                        component_ref: component_ref.into(),
                        component_id: component_id.as_str().into(),
                    }));
                }
            }
            "com.wasmcloud.lattice.component_scale_failed" => {
                let returned_component_ref =
                    get_string_data_from_json(&cloud_event.data, "image_ref")?;

                if returned_component_ref == component_ref {
                    let error = anyhow!(
                        "{}",
                        cloud_event
                            .data
                            .get("error")
                            .ok_or_else(|| anyhow!("No error found in data"))?
                            .as_str()
                            .ok_or_else(|| anyhow!("error is not a string"))?
                    );

                    return Ok(EventCheckOutcome::Failure(error));
                }
            }
            _ => {}
        }

        Ok(EventCheckOutcome::NotApplicable)
    };

    let event = find_event(receiver, timeout, check_function).await?;
    Ok(event)
}

/// Information related to an provider start
pub struct ProviderStartedInfo {
    pub host_id: String,
    pub provider_ref: String,
    pub provider_id: String,
}

/// Uses the NATS receiver to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
///
/// If the applicable provider start response event is found (either started or failed to start), the `Ok` variant of the `Result` will be returned,
/// with the `FindEventOutcome` enum containing the success or failure state of the event.
///
/// If the timeout is reached or another error occurs, the `Err` variant of the `Result` will be returned.
pub async fn wait_for_provider_start_event(
    receiver: &mut Receiver<Event>,
    timeout: Duration,
    host_id: String,
    provider_ref: String,
) -> Result<FindEventOutcome<ProviderStartedInfo>> {
    let check_function = move |event: Event| {
        let cloud_event = get_wasmbus_event_info(event)?;

        if cloud_event.source != host_id.as_str() {
            return Ok(EventCheckOutcome::NotApplicable);
        }

        match cloud_event.event_type.as_str() {
            "com.wasmcloud.lattice.provider_started" => {
                let image_ref = get_string_data_from_json(&cloud_event.data, "image_ref")?;

                if image_ref == provider_ref {
                    let provider_id = get_string_data_from_json(&cloud_event.data, "provider_id")?;

                    return Ok(EventCheckOutcome::Success(ProviderStartedInfo {
                        host_id: host_id.as_str().into(),
                        provider_ref: provider_ref.as_str().into(),
                        provider_id,
                    }));
                }
            }
            "com.wasmcloud.lattice.provider_start_failed" => {
                let returned_provider_ref =
                    get_string_data_from_json(&cloud_event.data, "provider_ref")?;

                if returned_provider_ref == provider_ref {
                    let error = anyhow!(
                        "{}",
                        cloud_event
                            .data
                            .get("error")
                            .ok_or_else(|| anyhow!("No error found in data"))?
                            .as_str()
                            .ok_or_else(|| anyhow!("error is not a string"))?
                    );

                    return Ok(EventCheckOutcome::Failure(error));
                }
            }
            _ => {}
        }

        Ok(EventCheckOutcome::NotApplicable)
    };

    let event = find_event(receiver, timeout, check_function).await?;
    Ok(event)
}

/// Information related to an provider stop
pub struct ProviderStoppedInfo {
    pub host_id: String,
    pub provider_id: String,
}

/// Uses the NATS receiver to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
///
/// If the applicable provider stop response event is found (either stopped or failed to stop), the `Ok` variant of the `Result` will be returned,
/// with the `FindEventOutcome` enum containing the success or failure state of the event.
///
/// If the timeout is reached or another error occurs, the `Err` variant of the `Result` will be returned.
pub async fn wait_for_provider_stop_event(
    receiver: &mut Receiver<Event>,
    timeout: Duration,
    host_id: String,
    provider_id: String,
) -> Result<FindEventOutcome<ProviderStoppedInfo>> {
    let check_function = move |event: Event| {
        let cloud_event = get_wasmbus_event_info(event)?;

        if cloud_event.source != host_id.as_str() {
            return Ok(EventCheckOutcome::NotApplicable);
        }

        match cloud_event.event_type.as_str() {
            "com.wasmcloud.lattice.provider_stopped" => {
                let returned_provider_id =
                    get_string_data_from_json(&cloud_event.data, "provider_id")?;

                if returned_provider_id == provider_id {
                    return Ok(EventCheckOutcome::Success(ProviderStoppedInfo {
                        host_id: host_id.as_str().into(),
                        provider_id: returned_provider_id,
                    }));
                }
            }
            "com.wasmcloud.lattice.provider_stop_failed" => {
                let returned_provider_id =
                    get_string_data_from_json(&cloud_event.data, "public_key")?;

                if returned_provider_id == provider_id {
                    let error = anyhow!(
                        "{}",
                        cloud_event
                            .data
                            .get("error")
                            .ok_or_else(|| anyhow!("No error found in data"))?
                            .as_str()
                            .ok_or_else(|| anyhow!("error is not a string"))?
                    );

                    return Ok(EventCheckOutcome::Failure(error));
                }
            }
            _ => {}
        }

        Ok(EventCheckOutcome::NotApplicable)
    };

    let event = find_event(receiver, timeout, check_function).await?;
    Ok(event)
}

/// Information related to an component stop
pub struct ComponentStoppedInfo {
    pub host_id: String,
    pub component_id: String,
}

/// Uses the NATS receiver to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
///
/// If the applicable stop component response event is found (either started or failed to start), the `Ok` variant of the `Result` will be returned,
/// with the `FindEventOutcome` enum containing the success or failure state of the event.
///
/// If the timeout is reached or another error occurs, the `Err` variant of the `Result` will be returned.
pub async fn wait_for_component_stop_event(
    receiver: &mut Receiver<Event>,
    timeout: Duration,
    host_id: String,
    component_id: String,
) -> Result<FindEventOutcome<ComponentStoppedInfo>> {
    let check_function = move |event: Event| {
        let cloud_event = get_wasmbus_event_info(event)?;

        if cloud_event.source != host_id.as_str() {
            return Ok(EventCheckOutcome::NotApplicable);
        }

        match cloud_event.event_type.as_str() {
            "com.wasmcloud.lattice.component_scaled" => {
                let returned_component_id =
                    get_string_data_from_json(&cloud_event.data, "public_key")?;
                if returned_component_id == component_id {
                    return Ok(EventCheckOutcome::Success(ComponentStoppedInfo {
                        host_id: host_id.as_str().into(),
                        component_id: returned_component_id,
                    }));
                }
            }
            "com.wasmcloud.lattice.component_scale_failed" => {
                let returned_component_id =
                    get_string_data_from_json(&cloud_event.data, "public_key")?;

                if returned_component_id == component_id {
                    let error = anyhow!(
                        "{}",
                        cloud_event
                            .data
                            .get("error")
                            .ok_or_else(|| anyhow!("No error found in data"))?
                            .as_str()
                            .ok_or_else(|| anyhow!("error is not a string"))?
                    );

                    return Ok(EventCheckOutcome::Failure(error));
                }
            }
            _ => {}
        }

        Ok(EventCheckOutcome::NotApplicable)
    };

    let event = find_event(receiver, timeout, check_function).await?;
    Ok(event)
}
