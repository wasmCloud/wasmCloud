use anyhow::{anyhow, bail, Result};
use cloudevents::{event::Event, AttributesReader};
use crossbeam_channel::Receiver;
use std::time::{Duration, Instant};

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
fn find_event<T>(
    receiver: &Receiver<Event>,
    timeout: Duration,
    check_function: impl Fn(Event) -> Result<EventCheckOutcome<T>>,
) -> Result<FindEventOutcome<T>> {
    let start = Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            bail!("Timeout waiting for event");
        }

        let event = receiver.recv_timeout(timeout - elapsed)?;

        let outcome = check_function(event)?;

        match outcome {
            EventCheckOutcome::Success(success_data) => {
                return Ok(FindEventOutcome::Success(success_data))
            }
            EventCheckOutcome::Failure(e) => return Ok(FindEventOutcome::Failure(e)),
            EventCheckOutcome::NotApplicable => continue,
        }
    }
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
enum EventCheckOutcome<T> {
    Success(T),
    Failure(anyhow::Error),
    NotApplicable,
}

/// Uses the NATS reciever to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
///
/// If the applicable actor start response event is found (either started or failed to start), the `Ok` variant of the `Result` will be returned,
/// with the `FindEventOutcome` enum containing the success or failure state of the event.
///
/// If the timeout is reached or another error occurs, the `Err` variant of the `Result` will be returned.
pub(crate) fn wait_for_actor_start_event(
    receiver: &Receiver<Event>,
    timeout: Duration,
    host_id: String,
    actor_ref: String,
) -> Result<FindEventOutcome<()>> {
    let check_function = move |event: Event| {
        let cloud_event = get_wasmbus_event_info(event)?;

        if cloud_event.source != host_id.as_str() {
            return Ok(EventCheckOutcome::NotApplicable);
        }

        match cloud_event.event_type.as_str() {
            "com.wasmcloud.lattice.actor_started" => {
                let image_ref = get_string_data_from_json(&cloud_event.data, "image_ref")?;

                if image_ref == actor_ref {
                    return Ok(EventCheckOutcome::Success(()));
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
                            .ok_or(anyhow!("No error found in data"))?
                            .as_str()
                            .ok_or(anyhow!("error is not a string"))?
                    );

                    return Ok(EventCheckOutcome::Failure(error));
                }
            }
            _ => {}
        }

        Ok(EventCheckOutcome::NotApplicable)
    };

    let event = find_event(receiver, timeout, check_function)?;
    Ok(event)
}

/// Uses the NATS reciever to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
///
/// If the applicable provider start response event is found (either started or failed to start), the `Ok` variant of the `Result` will be returned,
/// with the `FindEventOutcome` enum containing the success or failure state of the event.
///
/// If the timeout is reached or another error occurs, the `Err` variant of the `Result` will be returned.
pub fn wait_for_provider_start_event(
    receiver: &Receiver<Event>,
    timeout: Duration,
    host_id: String,
    provider_ref: String,
) -> Result<FindEventOutcome<()>> {
    let check_function = move |event: Event| {
        let cloud_event = get_wasmbus_event_info(event)?;

        if cloud_event.source != host_id.as_str() {
            return Ok(EventCheckOutcome::NotApplicable);
        }

        match cloud_event.event_type.as_str() {
            "com.wasmcloud.lattice.provider_started" => {
                let image_ref = get_string_data_from_json(&cloud_event.data, "image_ref")?;

                if image_ref == provider_ref {
                    return Ok(EventCheckOutcome::Success(()));
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
                            .ok_or(anyhow!("No error found in data"))?
                            .as_str()
                            .ok_or(anyhow!("error is not a string"))?
                    );

                    return Ok(EventCheckOutcome::Failure(error));
                }
            }
            _ => {}
        }

        Ok(EventCheckOutcome::NotApplicable)
    };

    let event = find_event(receiver, timeout, check_function)?;
    Ok(event)
}

/// Uses the NATS reciever to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
///
/// If the applicable provider stop response event is found (either stopped or failed to stop), the `Ok` variant of the `Result` will be returned,
/// with the `FindEventOutcome` enum containing the success or failure state of the event.
///
/// If the timeout is reached or another error occurs, the `Err` variant of the `Result` will be returned.
pub fn wait_for_provider_stop_event(
    receiver: &Receiver<Event>,
    timeout: Duration,
    host_id: String,
    provider_id: String,
) -> Result<FindEventOutcome<()>> {
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
                    return Ok(EventCheckOutcome::Success(()));
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
                            .ok_or(anyhow!("No error found in data"))?
                            .as_str()
                            .ok_or(anyhow!("error is not a string"))?
                    );

                    return Ok(EventCheckOutcome::Failure(error));
                }
            }
            _ => {}
        }

        Ok(EventCheckOutcome::NotApplicable)
    };

    let event = find_event(receiver, timeout, check_function)?;
    Ok(event)
}

/// Uses the NATS reciever to read events being published to the wasmCloud lattice event subject, up until the given timeout duration.
///
/// If the applicable stop actor response event is found (either started or failed to start), the `Ok` variant of the `Result` will be returned,
/// with the `FindEventOutcome` enum containing the success or failure state of the event.
///
/// If the timeout is reached or another error occurs, the `Err` variant of the `Result` will be returned.
pub fn wait_for_actor_stop_event(
    receiver: &Receiver<Event>,
    timeout: Duration,
    host_id: String,
    actor_id: String,
) -> Result<FindEventOutcome<()>> {
    let check_function = move |event: Event| {
        let cloud_event = get_wasmbus_event_info(event)?;

        if cloud_event.source != host_id.as_str() {
            return Ok(EventCheckOutcome::NotApplicable);
        }

        match cloud_event.event_type.as_str() {
            "com.wasmcloud.lattice.actor_stopped" => {
                let returned_actor_id = get_string_data_from_json(&cloud_event.data, "public_key")?;

                if returned_actor_id == actor_id {
                    return Ok(EventCheckOutcome::Success(()));
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
                            .ok_or(anyhow!("No error found in data"))?
                            .as_str()
                            .ok_or(anyhow!("error is not a string"))?
                    );

                    return Ok(EventCheckOutcome::Failure(error));
                }
            }
            _ => {}
        }

        Ok(EventCheckOutcome::NotApplicable)
    };

    let event = find_event(receiver, timeout, check_function)?;
    Ok(event)
}

/// Useful parts of a CloudEvent coming in from the wasmbus.
struct CloudEventData {
    event_type: String,
    source: String,
    data: serde_json::Value,
}

/// Get the useful parts out of a wasmbus cloud event.
fn get_wasmbus_event_info(event: Event) -> Result<CloudEventData> {
    let data: serde_json::Value = event
        .data()
        .ok_or(anyhow!("No data in event"))?
        .clone()
        .try_into()?;

    Ok(CloudEventData {
        event_type: event.ty().to_string(),
        source: event.source().to_string(),
        data,
    })
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
