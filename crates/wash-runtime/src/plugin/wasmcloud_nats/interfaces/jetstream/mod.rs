//! `wasmcloud:nats/jetstream@0.1.0` — the stream-level operations.
//!
//! The interface's two resources and the KV interface built on the same
//! JetStream context live beside this one: [`message_handle`],
//! [`pull_consumer`] and [`kv`].

use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::consumer::FromConsumer as _;
use futures::StreamExt as _;
use tracing::{debug, warn};
use wasmtime::component::{Accessor, Resource};

use crate::engine::ctx::{ActiveCtx, SharedCtx};

use super::{
    NatsId, check_payload, check_stream, check_subject, consumer_lookup_err, denied, jetstream_err,
    js, js_publish_err, labeled_js, nats_headers_to_wit, outbound_headers, stream_lookup_err,
    types, with_deadline,
};
use crate::plugin::wasmcloud_nats::conn::server_at_least;
use crate::plugin::wasmcloud_nats::jetstream::PullConsumerHandle;
use crate::plugin::wasmcloud_nats::policy::Denied;

pub(super) mod kv;
pub(super) mod message_handle;
pub(super) mod pull_consumer;

/// Upper bound on messages one `scan` may buffer into host memory.
const MAX_SCAN_MESSAGES: usize = 1_000;
/// Wall-clock bound on one `scan`, so a slow stream cannot pin the call open.
const MAX_SCAN_DURATION: Duration = Duration::from_secs(10);
/// Upper bound on subjects one `list-stream-subjects` may buffer.
///
/// Subject *cardinality* sizes the allocation here, not message count: a stream
/// on a per-order or per-device subject scheme holds millions of distinct
/// subjects, and draining the whole map would cost hundreds of megabytes of
/// host Strings before a list lift the guest would likely trap on.
const MAX_STREAM_SUBJECTS: usize = 1_000;
/// The server release that taught `$JS.API.STREAM.INFO` to honour a subjects
/// filter. Below it the field is ignored and the response simply carries no
/// subject map, which would otherwise reach the guest as an empty result.
const SUBJECT_FILTER_FLOOR: (u64, u64, u64) = (2, 7, 2);

/// Deletes `scan`'s ephemeral consumer if the host future never reaches its
/// in-line cleanup.
///
/// Under the concurrent ABI a guest can cancel a scan subtask, and a stopping
/// workload drops every host future it has in flight; either way the future is
/// abandoned at an await point and nothing after it runs. Without this the
/// consumer sits on the stream skewing `consumer_count` and pressuring
/// `max_consumers` until the server reaps it — exactly what a burst of
/// cancelled scans produces.
struct ScanConsumerGuard(Option<(jetstream::stream::Stream, String)>);

impl ScanConsumerGuard {
    fn arm(stream: &jetstream::stream::Stream, name: &str) -> Self {
        Self(Some((stream.clone(), name.to_string())))
    }

    /// Hands responsibility back once the in-line delete has completed, so the
    /// consumer is never deleted twice.
    fn defuse(&mut self) {
        self.0 = None;
    }
}

impl Drop for ScanConsumerGuard {
    fn drop(&mut self) {
        let Some((stream, name)) = self.0.take() else {
            return;
        };
        // Drop is synchronous and the delete is a round trip, so it has to
        // outlive this frame.
        tokio::spawn(async move {
            if let Err(e) = stream.delete_consumer(&name).await {
                warn!("failed to clean up scan consumer after cancellation: {e}");
            }
        });
    }
}

impl<T: 'static + Send> labeled_js::HostWithStore<T> for SharedCtx {
    async fn publish(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        msg: types::NatsMessage,
    ) -> wasmtime::Result<Result<js::PublishAck, types::NatsError>> {
        let conn = id;

        if let Err(e) = check_subject(&conn, &msg.subject) {
            return Ok(Err(e));
        }
        // JetStream answers a publish on a reply subject the client owns, so a
        // caller-supplied one has nowhere to go.
        if msg.reply_to.is_some() {
            debug!(
                subject = %msg.subject,
                "jetstream publish: caller-supplied reply-to is ignored"
            );
        }
        let headers = match outbound_headers(msg.headers.as_deref()) {
            Ok(h) => h,
            Err(e) => return Ok(Err(e)),
        };
        if let Err(e) = check_payload(msg.body.len(), headers.as_ref(), &conn) {
            return Ok(Err(e));
        }

        let ack_future = match headers {
            Some(headers) => {
                conn.jetstream
                    .publish_with_headers(msg.subject, headers, msg.body.into())
                    .await
            }
            None => conn.jetstream.publish(msg.subject, msg.body.into()).await,
        };
        let ack_future = match ack_future {
            Ok(f) => f,
            Err(e) => {
                return Ok(Err(js_publish_err(
                    "failed to publish",
                    e,
                    conn.max_payload(),
                )));
            }
        };

        match ack_future.await {
            Ok(ack) => Ok(Ok(js::PublishAck {
                stream_name: ack.stream,
                sequence: ack.sequence,
                duplicate: ack.duplicate,
            })),
            Err(e) => Ok(Err(js_publish_err(
                "failed to confirm publish",
                e,
                conn.max_payload(),
            ))),
        }
    }

    async fn get_by_sequence(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        stream_name: String,
        sequence: u64,
    ) -> wasmtime::Result<Result<js::StoredMessage, types::NatsError>> {
        let conn = id;
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }
        if !conn.server_at_least(2, 9) {
            return Ok(Err(types::NatsError::UnsupportedByServer(
                "direct get requires NATS server 2.9 or newer".to_string(),
            )));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
        };

        match stream.direct_get(sequence).await {
            Ok(m) => {
                // The stream grant got us this far; the subject grant decides
                // whether this particular message is one the workload may
                // read, the same boundary a consumer filter is held to. The
                // refusal names the sequence rather than the subject it found
                // there, so a caller cannot walk sequences to learn the names
                // of the subjects it was not granted.
                if conn.policy.check_stored_subject(&m.subject).is_err() {
                    return Ok(Err(denied(
                        Denied::NotGranted,
                        types::DeniedResource::Message(sequence),
                        &stream_name,
                    )));
                }
                Ok(Ok(js::StoredMessage {
                    subject: m.subject.to_string(),
                    sequence: m.sequence,
                    data: m.payload.to_vec(),
                    headers: Some(nats_headers_to_wit(&m.headers)),
                }))
            }
            Err(e) => Ok(Err(jetstream_err("get-by-sequence failed", e))),
        }
    }

    async fn scan(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        stream_name: String,
        start_sequence: u64,
        max_count: u32,
    ) -> wasmtime::Result<Result<Vec<js::StoredMessage>, types::NatsError>> {
        let conn = id;
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
        };

        let effective_start = start_sequence.max(1);
        let pull_consumer = match stream
            .create_consumer(jetstream::consumer::pull::Config {
                deliver_policy: jetstream::consumer::DeliverPolicy::ByStartSequence {
                    start_sequence: effective_start,
                },
                // Belt to the braces of the cleanup below: anything that still
                // escapes both the in-line delete and the drop guard — a host
                // that dies mid-scan — is reaped by the server instead of
                // counting against the stream's `max_consumers` forever. Sits
                // comfortably above MAX_SCAN_DURATION so a slow but healthy
                // scan is never reaped out from under itself.
                inactive_threshold: Duration::from_secs(30),
                ..Default::default()
            })
            .await
        {
            Ok(c) => c,
            Err(e) => return Ok(Err(jetstream_err("failed to create scan consumer", e))),
        };
        // The delete below only runs if this future is still being polled. A
        // guest that cancels the scan subtask, or a workload that stops, drops
        // it at an await point instead, and the guard is what deletes the
        // consumer on that path.
        let mut cleanup = ScanConsumerGuard::arm(&stream, &pull_consumer.cached_info().name);

        // Read inside a block so every exit — including the error ones — falls
        // through to the cleanup below with the message stream already dropped.
        let collected: Result<Vec<js::StoredMessage>, types::NatsError> = async {
            let mut msg_stream = pull_consumer
                .messages()
                .await
                .map_err(|e| jetstream_err("failed to get messages", e))?;

            let mut messages = Vec::new();
            let mut denied_subjects = 0usize;
            let limit = (max_count as usize).min(MAX_SCAN_MESSAGES);
            let deadline = tokio::time::Instant::now() + MAX_SCAN_DURATION;

            while messages.len() < limit && tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_millis(100), msg_stream.next()).await {
                    Ok(Some(Ok(msg))) => {
                        // A stream grant is authority over the stream, not
                        // over every subject stored in it: the declarative
                        // path holds a consumer's filter to `subject-allow`,
                        // and reading the same messages directly answers the
                        // same way. Skipped messages are not counted against
                        // the limit, so the result is what a server-side
                        // filtered consumer would have delivered — a full
                        // batch whose sequences may gap.
                        if conn.policy.check_stored_subject(&msg.subject).is_err() {
                            denied_subjects += 1;
                            continue;
                        }
                        let sequence = msg
                            .info()
                            .map_err(|e| jetstream_err("failed to get message info", e))?
                            .stream_sequence;
                        messages.push(js::StoredMessage {
                            subject: msg.subject.to_string(),
                            sequence,
                            data: msg.payload.to_vec(),
                            headers: msg.headers.as_ref().map(nats_headers_to_wit),
                        });
                    }
                    Ok(Some(Err(e))) => {
                        warn!("error reading message: {e}");
                        break;
                    }
                    Ok(None) | Err(_) => break,
                }
            }
            // Same hazard as `list-stream-subjects`: a scan that returns
            // nothing *because of the grant* reads as an empty range of the
            // stream, and neither the guest nor the operator can tell. Every
            // other refusal on this path is typed; this one has to be too.
            if messages.is_empty() && denied_subjects > 0 {
                warn!(
                    stream = %stream_name,
                    denied = denied_subjects,
                    subject_allow = %conn.policy.granted_subject_patterns().join(","),
                    "scan read {denied_subjects} messages, every one of them on a subject \
                     outside this workload's `subject-allow`; refusing rather than returning \
                     an empty batch that reads as an empty range"
                );
                return Err(denied(
                    Denied::NotGranted,
                    types::DeniedResource::Subject,
                    &stream_name,
                ));
            }
            if denied_subjects > 0 {
                debug!(
                    stream = %stream_name,
                    denied = denied_subjects,
                    returned = messages.len(),
                    "scan omitted messages on subjects outside the workload's grant"
                );
            }
            Ok(messages)
        }
        .await;

        // The consumer is ephemeral to this call; without this each scan leaves
        // one behind on the stream.
        if let Err(e) = stream
            .delete_consumer(&pull_consumer.cached_info().name)
            .await
        {
            warn!("failed to clean up scan consumer: {e}");
        }
        cleanup.defuse();

        let mut messages = match collected {
            Ok(m) => m,
            Err(e) => return Ok(Err(e)),
        };
        messages.sort_by_key(|m| m.sequence);
        Ok(Ok(messages))
    }

    async fn open_pull_consumer(
        accessor: &Accessor<T, Self>,
        id: NatsId,
        stream_name: String,
        consumer: String,
    ) -> wasmtime::Result<Result<Resource<PullConsumerHandle>, types::NatsError>> {
        let conn = id;
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
        };

        // Attach only: `create_consumer` is create-or-update and would rewrite
        // an existing durable's filter subject and deliver policy.
        //
        // Spelled out rather than left to `get_consumer`, which folds the
        // lookup and the config conversion into one `crate::Error` and erases
        // the difference between "no such consumer", "the API did not answer",
        // and "it exists but it is a push consumer" — three conditions a guest
        // has to tell apart, and only the first of which is `not-found`.
        let info = match stream.consumer_info(&consumer).await {
            Ok(i) => i,
            Err(e) => return Ok(Err(consumer_lookup_err(&stream_name, &consumer, e))),
        };
        // A durable is provisioned out of band, so its filter is the operator's
        // and not this workload's to trust: attaching to one filtered
        // `orders.>` under a `subject-allow` of `orders.mine` would deliver
        // every subject in the stream. Refused at the attach, which is a much
        // better place to fail than per delivery, and the same boundary a
        // declarative `subscriptions:` entry is held to.
        for filter in consumer_filters(&info.config) {
            if let Err(reason) = conn.policy.check_filter(filter) {
                return Ok(Err(denied(reason, types::DeniedResource::Subject, filter)));
            }
        }

        let config = match jetstream::consumer::pull::Config::try_from_consumer_config(
            info.config.clone(),
        ) {
            Ok(c) => c,
            Err(e) => {
                return Ok(Err(types::NatsError::Jetstream(format!(
                    "consumer '{consumer}' on stream '{stream_name}' exists but is not a pull consumer: {e}"
                ))));
            }
        };
        let opened = jetstream::consumer::Consumer::new(config, info, conn.jetstream.clone());

        let resource = accessor.with(|mut a| {
            a.get().table.push(PullConsumerHandle {
                consumer: opened,
                max_fetch_bytes: u64::try_from(conn.limits.subscription_capacity_bytes)
                    .unwrap_or(u64::MAX),
                budget: conn.fetch_budget.clone(),
                policy: conn.policy.clone(),
            })
        })?;
        Ok(Ok(resource))
    }

    async fn get_stream_info(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        stream_name: String,
    ) -> wasmtime::Result<Result<js::StreamInfo, types::NatsError>> {
        let conn = id;
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
        };
        let info = stream.cached_info();
        Ok(Ok(js::StreamInfo {
            name: info.config.name.clone(),
            subjects: info.config.subjects.clone(),
            messages: info.state.messages,
            bytes: info.state.bytes,
            first_sequence: info.state.first_sequence,
            last_sequence: info.state.last_sequence,
            consumer_count: info.state.consumer_count as u64,
        }))
    }

    async fn list_stream_subjects(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        stream_name: String,
        subject_filter: String,
    ) -> wasmtime::Result<Result<Vec<js::SubjectCount>, types::NatsError>> {
        let conn = id;
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }
        // An older server does not reject the subjects filter, it ignores it
        // and answers with no subject map — which would reach the guest as an
        // authoritative empty list. Say the call is unavailable instead.
        let version = conn.server_version();
        if !server_at_least(&version, SUBJECT_FILTER_FLOOR) {
            let (major, minor, patch) = SUBJECT_FILTER_FLOOR;
            return Ok(Err(types::NatsError::UnsupportedByServer(format!(
                "subject filtering of stream info requires NATS server {major}.{minor}.{patch} or \
                 newer; connected server is {version}"
            ))));
        }

        // Every await below is bounded and every outcome is logged. This call
        // was reported as returning nothing, erroring nothing and logging
        // nothing at three stream widths — indistinguishable from the guest
        // never having called — and the first STREAM.INFO was the one await
        // with no bound of its own. See `with_deadline`.
        let stream = match with_deadline(
            &conn,
            "jetstream.list-stream-subjects lookup",
            &stream_name,
            MAX_SCAN_DURATION,
            conn.jetstream.get_stream(&stream_name),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
            Err(timeout) => return Ok(Err(timeout)),
        };

        let filter = if subject_filter.is_empty() {
            ">".to_string()
        } else {
            subject_filter
        };
        // `info_with_subjects` issues the first STREAM.INFO eagerly, and a
        // subject map over hundreds of thousands of subjects is the slowest
        // request this interface makes.
        let mut info = match with_deadline(
            &conn,
            "jetstream.list-stream-subjects info",
            &stream_name,
            MAX_SCAN_DURATION,
            stream.info_with_subjects(&filter),
        )
        .await
        {
            Ok(Ok(i)) => i,
            Ok(Err(e)) => return Ok(Err(jetstream_err("stream info failed", e))),
            Err(timeout) => return Ok(Err(timeout)),
        };

        // Bounded on both axes, as every other collecting endpoint here is:
        // the paging walk keeps the call open for as long as the server has
        // pages, and the subject map is the one result whose size the guest's
        // arguments do not cap.
        let mut out = Vec::new();
        let mut denied_subjects = 0usize;
        let mut truncated = None;
        let deadline =
            tokio::time::Instant::now() + conn.request_timeout.unwrap_or(MAX_SCAN_DURATION);
        loop {
            match tokio::time::timeout_at(deadline, info.next()).await {
                Ok(Some(Ok((subject, count)))) => {
                    // Stream-level introspection is subject-scoped like every
                    // other read here: `stream-allow` alone would hand back the
                    // names of subjects `subject-allow` refuses to deliver, and
                    // enumerating them is what the `denied-resource::message`
                    // refusal is careful not to do. Skipped rather than
                    // refused, so a wide stream still answers for the part of
                    // it this workload was granted — but counted, because an
                    // empty result the grant produced must not read as an
                    // empty stream. See below.
                    if conn.policy.check_stored_subject(&subject).is_err() {
                        denied_subjects += 1;
                        continue;
                    }
                    out.push(js::SubjectCount {
                        subject,
                        count: count as u64,
                    });
                    if out.len() >= MAX_STREAM_SUBJECTS {
                        truncated = Some("entry limit");
                        break;
                    }
                }
                Ok(Some(Err(e))) => return Ok(Err(jetstream_err("stream subjects failed", e))),
                Ok(None) => break,
                Err(_) => {
                    truncated = Some("deadline");
                    break;
                }
            }
        }
        out.sort_by(|a, b| a.subject.cmp(&b.subject));

        // A list that is empty *because of the grant* is the same failure the
        // server-version floor above is careful to avoid: it reaches the guest
        // as an authoritative "this stream holds nothing" when the truth is
        // "you may not see what it holds". Both are silent, and this one is
        // silent even in the host log.
        if out.is_empty() && denied_subjects > 0 {
            warn!(
                stream = %stream_name,
                filter = %filter,
                denied = denied_subjects,
                subject_allow = %conn.policy.granted_subject_patterns().join(","),
                "list-stream-subjects matched {denied_subjects} subjects, every one of them \
                 outside this workload's `subject-allow`; refusing rather than returning an \
                 empty list that reads as an empty stream"
            );
            return Ok(Err(denied(
                Denied::NotGranted,
                types::DeniedResource::Subject,
                &filter,
            )));
        }
        if denied_subjects > 0 {
            debug!(
                stream = %stream_name,
                filter = %filter,
                denied = denied_subjects,
                returned = out.len(),
                "list-stream-subjects omitted subjects outside the workload's grant"
            );
        }

        match truncated {
            // A truncated list is not a list: a guest reading it as
            // authoritative draws the wrong conclusion about what the stream
            // holds and has no way to tell. An error it can see beats a WARN
            // only the operator can.
            Some(reason) => {
                warn!(
                    stream = %stream_name,
                    filter = %filter,
                    collected = out.len(),
                    reason,
                    "list-stream-subjects could not enumerate the whole subject map; \
                     returning an error rather than a partial list the guest would read \
                     as complete"
                );
                Ok(Err(types::NatsError::LimitExceeded(format!(
                    "stream '{stream_name}' has more subjects matching '{filter}' than one \
                     call can return ({} collected, stopped by the {reason}); narrow \
                     `subject-filter`",
                    out.len(),
                ))))
            }
            None => {
                debug!(
                    stream = %stream_name,
                    filter = %filter,
                    subjects = out.len(),
                    "list-stream-subjects returning"
                );
                Ok(Ok(out))
            }
        }
    }

    async fn get_consumer_info(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        stream_name: String,
        consumer: String,
    ) -> wasmtime::Result<Result<js::ConsumerInfo, types::NatsError>> {
        let conn = id;
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
        };
        match stream.consumer_info(&consumer).await {
            Ok(info) => {
                // Same boundary `open_pull_consumer` holds a consumer to, for
                // the same reason: a filter subject is a subject name, and a
                // binding that may not read those messages may not learn the
                // names either.
                for filter in consumer_filters(&info.config) {
                    if let Err(reason) = conn.policy.check_filter(filter) {
                        return Ok(Err(denied(reason, types::DeniedResource::Subject, filter)));
                    }
                }
                Ok(Ok(consumer_info_to_wit(&info)))
            }
            Err(e) => Ok(Err(consumer_lookup_err(&stream_name, &consumer, e))),
        }
    }
}

impl js::Host for ActiveCtx<'_> {}

/// The subjects a consumer's config filters on, as the grant check sees them.
///
/// A consumer with neither filter set captures the stream's whole subject
/// space, which is `>`.
fn consumer_filters(config: &jetstream::consumer::Config) -> Vec<&str> {
    if !config.filter_subject.is_empty() {
        vec![config.filter_subject.as_str()]
    } else if !config.filter_subjects.is_empty() {
        config.filter_subjects.iter().map(String::as_str).collect()
    } else {
        vec![">"]
    }
}

fn consumer_info_to_wit(info: &jetstream::consumer::Info) -> js::ConsumerInfo {
    js::ConsumerInfo {
        name: info.name.clone(),
        stream_name: info.stream_name.clone(),
        filter_subject: info.config.filter_subject.clone(),
        // Reported alongside the singular field rather than folded into it: a
        // consumer provisioned with the plural form leaves the singular one
        // empty, and empty is how the WIT says "captures the whole stream".
        // Joining the list into one string would only mislead subject parsers.
        filter_subjects: info.config.filter_subjects.clone(),
        max_ack_pending: info.config.max_ack_pending.max(0) as u64,
        max_waiting: info.config.max_waiting.max(0) as u64,
        // The two a guest has to size a pull against: over either, the server
        // refuses the request outright rather than trimming it.
        max_request_batch: info.config.max_batch.max(0) as u64,
        max_request_max_bytes: info.config.max_bytes.max(0) as u64,
        max_deliver: info.config.max_deliver.max(0) as u64,
        ack_wait_ms: info.config.ack_wait.as_millis().min(u64::MAX as u128) as u64,
        num_ack_pending: info.num_ack_pending as u64,
        num_pending: info.num_pending,
        num_redelivered: info.num_redelivered as u64,
    }
}
