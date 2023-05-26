use anyhow::{anyhow, bail, Result};
use cloudevents::event::{AttributesReader, Event};
use tokio::sync::mpsc::Receiver;
use tokio::time::{Duration, Instant};

/// Useful parts of a CloudEvent coming in from the wasmbus.
#[derive(Debug)]
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
/// It can either succeed or fail. This enum should only be returned if we found the applicable event.
/// If we did not find the event or another error occured, use the `Err` variant of a `Result` wrapping around this enum.
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

/// Uses the NATS reciever to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
///
/// Takes a `check_function`, which recieves each event coming in from the receiver. This function must return a `Result<EventCheckOutcome>`.
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
                    "Channel dropped before event was received, please report this at https://github.com/wasmCloud/wash/issues with details to reproduce"
                )))
            }

        }
    }
}

/// Information related to an actor start
pub struct ActorStartedInfo {
    pub host_id: String,
    pub actor_ref: String,
    pub actor_id: String,
}

/// Uses the NATS reciever to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
///
/// If the applicable actor start response event is found (either started or failed to start), the `Ok` variant of the `Result` will be returned,
/// with the `FindEventOutcome` enum containing the success or failure state of the event.
///
/// If the timeout is reached or another error occurs, the `Err` variant of the `Result` will be returned.
pub async fn wait_for_actor_start_event(
    receiver: &mut Receiver<Event>,
    timeout: Duration,
    host_id: String,
    actor_ref: String,
) -> Result<FindEventOutcome<ActorStartedInfo>> {
    let check_function = move |event: Event| {
        let cloud_event = get_wasmbus_event_info(event)?;

        if cloud_event.source != host_id.as_str() {
            return Ok(EventCheckOutcome::NotApplicable);
        }

        match cloud_event.event_type.as_str() {
            "com.wasmcloud.lattice.actor_started" => {
                let image_ref = get_string_data_from_json(&cloud_event.data, "image_ref")?;

                if image_ref == actor_ref {
                    let actor_id = get_string_data_from_json(&cloud_event.data, "public_key")?;
                    return Ok(EventCheckOutcome::Success(ActorStartedInfo {
                        host_id: host_id.as_str().into(),
                        actor_ref: actor_ref.as_str().into(),
                        actor_id,
                    }));
                }
            }
            "com.wasmcloud.lattice.actor_start_failed" => {
                let returned_actor_ref = get_string_data_from_json(&cloud_event.data, "actor_ref")?;

                if returned_actor_ref == actor_ref {
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
    pub link_name: String,
    pub contract_id: String,
}

/// Uses the NATS reciever to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
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
                    let provider_id = get_string_data_from_json(&cloud_event.data, "public_key")?;
                    let contract_id = get_string_data_from_json(&cloud_event.data, "contract_id")?;
                    let link_name = get_string_data_from_json(&cloud_event.data, "link_name")?;

                    return Ok(EventCheckOutcome::Success(ProviderStartedInfo {
                        host_id: host_id.as_str().into(),
                        provider_ref: provider_ref.as_str().into(),
                        provider_id,
                        contract_id,
                        link_name,
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
    pub link_name: String,
    pub contract_id: String,
}

/// Uses the NATS reciever to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
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
                    get_string_data_from_json(&cloud_event.data, "public_key")?;

                if returned_provider_id == provider_id {
                    let contract_id = get_string_data_from_json(&cloud_event.data, "contract_id")?;
                    let link_name = get_string_data_from_json(&cloud_event.data, "link_name")?;

                    return Ok(EventCheckOutcome::Success(ProviderStoppedInfo {
                        host_id: host_id.as_str().into(),
                        provider_id: returned_provider_id,
                        contract_id,
                        link_name,
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

/// Information related to an actor stop
pub struct ActorStoppedInfo {
    pub host_id: String,
    pub actor_id: String,
}

/// Uses the NATS reciever to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
///
/// If the applicable stop actor response event is found (either started or failed to start), the `Ok` variant of the `Result` will be returned,
/// with the `FindEventOutcome` enum containing the success or failure state of the event.
///
/// If the timeout is reached or another error occurs, the `Err` variant of the `Result` will be returned.
pub async fn wait_for_actor_stop_event(
    receiver: &mut Receiver<Event>,
    timeout: Duration,
    host_id: String,
    actor_id: String,
) -> Result<FindEventOutcome<ActorStoppedInfo>> {
    let check_function = move |event: Event| {
        let cloud_event = get_wasmbus_event_info(event)?;

        if cloud_event.source != host_id.as_str() {
            return Ok(EventCheckOutcome::NotApplicable);
        }

        match cloud_event.event_type.as_str() {
            "com.wasmcloud.lattice.actor_stopped" => {
                let returned_actor_id = get_string_data_from_json(&cloud_event.data, "public_key")?;
                if returned_actor_id == actor_id {
                    return Ok(EventCheckOutcome::Success(ActorStoppedInfo {
                        host_id: host_id.as_str().into(),
                        actor_id: returned_actor_id,
                    }));
                }
            }
            "com.wasmcloud.lattice.actor_stop_failed" => {
                let returned_actor_id = get_string_data_from_json(&cloud_event.data, "public_key")?;

                if returned_actor_id == actor_id {
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
