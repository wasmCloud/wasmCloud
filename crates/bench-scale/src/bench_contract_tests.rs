use super::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;

use clap::Parser as _;
use serde_json::{Value, json};
use testcontainers::{
    ContainerAsync, GenericImage,
    core::{IntoContainerPort as _, WaitFor},
    runners::AsyncRunner as _,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::time::timeout;
use wash_runtime::washlet::rpc_subject;

enum Reply {
    State(WorkloadState, &'static str),
    Bytes(&'static [u8]),
    Silent,
}

#[derive(Default)]
struct Script {
    replies: BTreeMap<&'static str, VecDeque<Reply>>,
    requests: Vec<(String, Value)>,
}

fn status_reply(state: WorkloadState, message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "workloadStatus": {"workloadState": state.as_str_name(), "message": message}
    }))
    .unwrap()
}

fn category(started: &Started) -> (&'static str, &str) {
    match started {
        Started::Running => ("running", ""),
        Started::Refused(message) => ("refused", message.as_str()),
        Started::Unanswered(message) => ("unanswered", message.as_str()),
        Started::Failed(message) => ("failed", message.as_str()),
        Started::Skipped => ("skipped", ""),
    }
}

struct Fixture {
    client: HostClient,
    nats: async_nats::Client,
    url: String,
    id: String,
    script: Arc<Mutex<Script>>,
    task: tokio::task::JoinHandle<()>,
    _server: Option<ContainerAsync<GenericImage>>,
}

impl Fixture {
    async fn new() -> Result<Self> {
        let (url, server) = if let Ok(url) = std::env::var("NATS_URL") {
            (url, None)
        } else {
            let server = GenericImage::new("nats", "2.12.8-alpine")
                .with_exposed_port(4222.tcp())
                .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
                .start()
                .await?;
            (
                format!(
                    "nats://127.0.0.1:{}",
                    server.get_host_port_ipv4(4222).await?
                ),
                Some(server),
            )
        };
        let nats = timeout(Duration::from_secs(30), async {
            loop {
                match async_nats::connect(&url).await {
                    Ok(client) => break client,
                    Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                }
            }
        })
        .await?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let prefix = rpc_subject(&id, "");
        let mut subscription = nats.subscribe(rpc_subject(&id, ">")).await?;
        nats.flush().await?;
        let script = Arc::new(Mutex::new(Script::default()));
        let task = tokio::spawn({
            let script = Arc::clone(&script);
            let nats = nats.clone();
            let id = id.clone();
            async move {
                while let Some(message) = subscription.next().await {
                    let Some(reply_to) = message.reply else {
                        continue;
                    };
                    let command = message.subject.strip_prefix(&prefix).unwrap().to_string();
                    let reply = {
                        let mut script = script.lock().unwrap();
                        script.requests.push((
                            command.clone(),
                            serde_json::from_slice(&message.payload).unwrap(),
                        ));
                        script
                            .replies
                            .get_mut(command.as_str())
                            .and_then(VecDeque::pop_front)
                    };
                    let bytes = match (reply, command.as_str()) {
                        (None, "heartbeat") => {
                            serde_json::to_vec(&json!({"id": id, "version": "contract-test"}))
                                .unwrap()
                        }
                        (Some(Reply::Silent), _) => continue,
                        (Some(Reply::Bytes(bytes)), _) => bytes.to_vec(),
                        (Some(Reply::State(state, message)), _) => status_reply(state, message),
                        (None, "workload.start") => status_reply(WorkloadState::Running, ""),
                        (None, "workload.stop") => status_reply(WorkloadState::Completed, ""),
                        (None, _) => status_reply(WorkloadState::NotFound, ""),
                    };
                    nats.publish(reply_to, bytes.into()).await.unwrap();
                }
            }
        });
        Ok(Self {
            client: HostClient::new(nats.clone(), id.clone()),
            nats,
            url,
            id,
            script,
            task,
            _server: server,
        })
    }

    fn replies(&self, command: &'static str, replies: Vec<Reply>) {
        self.script
            .lock()
            .unwrap()
            .replies
            .insert(command, replies.into());
    }

    fn workload_ids(&self, command: &str) -> BTreeSet<String> {
        self.script
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|(sent, _)| sent == command)
            .filter_map(|(_, request)| Some(request.get("workloadId")?.as_str()?.to_string()))
            .collect()
    }

    fn args(&self, http: &HttpFixture) -> Args {
        Args::try_parse_from([
            "bench-scale",
            "--nats-url",
            &self.url,
            "--host-id",
            &self.id,
            "--host-http",
            &http.address,
            "--image",
            "example.invalid/http:test",
            "--memory-limit",
            "1TiB",
            "--max-components",
            "2",
            "--step",
            "1",
            "--sample",
            "2",
            "--start-concurrency",
            "1",
            "--request-concurrency",
            "1",
            "--start-timeout",
            "1",
            "--request-timeout",
            "1",
            "--start-retries",
            "0",
            "--spin-down-wait",
            "0",
            "--report",
        ])
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct HttpFixture {
    address: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl HttpFixture {
    async fn new(status: u16) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?.to_string();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let task = tokio::spawn({
            let requests = Arc::clone(&requests);
            async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let requests = Arc::clone(&requests);
                    tokio::spawn(async move {
                        let mut head = Vec::new();
                        while !head.ends_with(b"\r\n\r\n") {
                            let Ok(byte) = socket.read_u8().await else {
                                return;
                            };
                            head.push(byte);
                            if head.len() >= 8192 {
                                return;
                            }
                        }
                        let Ok(head) = String::from_utf8(head) else {
                            return;
                        };
                        requests.lock().unwrap().push(head);
                        let response = format!(
                            "HTTP/1.1 {status} Test\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntest"
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                    });
                }
            }
        });
        Ok(Self {
            address,
            requests,
            task,
        })
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[test]
fn dynamic_cap_overshoots_and_never_undershoots() {
    for (theoretical, overshoot, cap) in [
        (10, 1.1, 11),
        (7, 1.1, 8),
        (10, 1.25, 13),
        (10, 0.5, 10),
        (0, 2.0, 0),
        (3, 1.0, 3),
    ] {
        assert_eq!(
            dynamic_cap(theoretical, overshoot),
            cap,
            "{theoretical} x {overshoot}"
        );
    }
}

#[test]
fn stop_displays_leave_every_diagnostic_out() {
    for (stop, display) in [
        (
            Stop::Requests {
                stage: "warm-up",
                failed: 1,
                total: 1,
                example: Some("body=hunter2".into()),
            },
            "requests failing: 1 of 1 warm-up requests",
        ),
        (
            Stop::StartsFailed {
                count: 3,
                example: "hunter2".into(),
            },
            "3 starts failed",
        ),
        (
            Stop::StartsUnanswered {
                count: 2,
                example: "hunter2".into(),
            },
            "2 starts unanswered",
        ),
    ] {
        assert_eq!(stop.to_string(), display);
        assert!(!stop.to_string().contains("hunter2"));
        assert!(stop.diagnostic().is_some_and(|d| d.contains("hunter2")));
    }
    for (stop, display) in [
        (
            Stop::Requests {
                stage: "sampled",
                failed: 2,
                total: 4,
                example: None,
            },
            "requests failing: 2 of 4 sampled requests",
        ),
        (
            Stop::MemoryLimit {
                footprint: 3 << 30,
                limit: 2 << 30,
            },
            "memory limit: footprint 3GiB >= 2GiB",
        ),
        (Stop::Cap { cap: 5 }, "cap of 5 workloads reached"),
    ] {
        assert_eq!(stop.to_string(), display);
        assert_eq!(stop.diagnostic(), None);
    }
}

#[test]
fn rpc_errors_display_their_cause() {
    assert_eq!(RpcError::Timeout.to_string(), "no reply before the timeout");
    assert_eq!(RpcError::NoResponders.to_string(), "no responders");
    assert_eq!(RpcError::Other(anyhow::anyhow!("x")).to_string(), "x");
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn rpc_contract_reports_states_protocol_errors_and_no_responders() -> Result<()> {
    let fixture = Fixture::new().await?;
    assert_eq!(
        fixture
            .client
            .heartbeat(Duration::from_secs(1))
            .await
            .unwrap()
            .id,
        fixture.id
    );
    for state in [
        WorkloadState::Running,
        WorkloadState::Starting,
        WorkloadState::Error,
        WorkloadState::Stopping,
        WorkloadState::NotFound,
    ] {
        fixture.replies("workload.status", vec![Reply::State(state, "diagnostic")]);
        let status = fixture
            .client
            .status("workload-id", Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(status.state, state);
        assert_eq!(status.message, "diagnostic");
    }
    fixture.replies("workload.status", vec![Reply::Bytes(b"not JSON")]);
    assert!(matches!(
        fixture
            .client
            .status("workload-id", Duration::from_secs(1))
            .await,
        Err(RpcError::Other(_))
    ));
    fixture.replies("workload.status", vec![Reply::Silent]);
    assert!(matches!(
        fixture
            .client
            .status("workload-id", Duration::from_millis(20))
            .await,
        Err(RpcError::Timeout)
    ));
    let absent = HostClient::new(fixture.nats.clone(), "absent-host".into());
    assert!(matches!(
        absent.status("workload-id", Duration::from_secs(1)).await,
        Err(RpcError::NoResponders)
    ));
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn starts_settle_retries_and_preserve_failure_categories() -> Result<()> {
    let fixture = Fixture::new().await?;
    let starting = || Reply::State(WorkloadState::Starting, "");
    let running = || Reply::State(WorkloadState::Running, "");
    let not_found = || Reply::State(WorkloadState::NotFound, "");
    for (starts, statuses, retries, expected, fragment) in [
        (vec![running()], vec![], 0, "running", ""),
        (vec![starting()], vec![running()], 0, "running", ""),
        (
            vec![Reply::State(WorkloadState::Unspecified, "")],
            vec![running()],
            0,
            "running",
            "",
        ),
        (vec![Reply::Silent], vec![running()], 0, "running", ""),
        (
            vec![starting()],
            vec![Reply::Bytes(b"invalid"), running()],
            0,
            "running",
            "",
        ),
        (
            vec![Reply::State(WorkloadState::Error, "memory limit")],
            vec![],
            0,
            "refused",
            "memory limit",
        ),
        (
            vec![Reply::Bytes(b"invalid")],
            vec![],
            0,
            "failed",
            "undecodable reply",
        ),
        (
            vec![Reply::State(WorkloadState::Stopping, "stopping")],
            vec![],
            0,
            "failed",
            "Stopping: stopping",
        ),
        (
            vec![starting()],
            vec![Reply::State(WorkloadState::Stopping, "stopping")],
            0,
            "failed",
            "Stopping: stopping",
        ),
        (
            vec![starting()],
            vec![not_found()],
            0,
            "unanswered",
            "no answer after 1 attempts (0 timeouts, 0 no responders)",
        ),
        (
            vec![Reply::Silent],
            vec![not_found()],
            0,
            "unanswered",
            "no answer after 1 attempts (1 timeouts, 0 no responders)",
        ),
        (
            vec![starting()],
            std::iter::repeat_with(starting).take(64).collect(),
            0,
            "unanswered",
            "still not running",
        ),
        (
            vec![
                Reply::Silent,
                Reply::State(WorkloadState::Error, "duplicate"),
            ],
            vec![not_found(), running()],
            1,
            "running",
            "",
        ),
        (
            vec![
                Reply::Silent,
                Reply::State(WorkloadState::Error, "duplicate"),
            ],
            vec![not_found(), Reply::State(WorkloadState::Error, "refused")],
            1,
            "refused",
            "duplicate",
        ),
    ] {
        // A silent reply costs the whole timeout; every other reply arrives
        // over a real NATS round trip and must not be mistaken for one.
        let silent = starts
            .iter()
            .chain(&statuses)
            .any(|reply| matches!(reply, Reply::Silent));
        let start_timeout = if silent {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(2)
        };
        fixture.replies("workload.start", starts);
        fixture.replies("workload.status", statuses);
        let halt = AtomicBool::new(false);
        let journal = Journal::default();
        let starter = Starter {
            client: &fixture.client,
            image: "example.invalid/http:test",
            flavour: Flavour::Warm,
            run_id: "retry",
            timeout: start_timeout,
            retries,
            halt: &halt,
            journal: &journal,
        };
        let (result, _) = starter.start(0).await;
        let (actual, message) = category(&result);
        assert_eq!(actual, expected, "{message}");
        assert!(message.contains(fragment), "{expected}: {message}");
        assert_eq!(
            halt.load(Ordering::Relaxed),
            matches!(result, Started::Refused(_) | Started::Unanswered(_))
        );
        if halt.load(Ordering::Relaxed) {
            assert!(matches!(starter.start(1).await.0, Started::Skipped));
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn no_responders_back_off_then_halt() -> Result<()> {
    let fixture = Fixture::new().await?;
    let absent = HostClient::new(fixture.nats.clone(), "absent-host".into());
    let halt = AtomicBool::new(false);
    let journal = Journal::default();
    let starter = Starter {
        client: &absent,
        image: "example.invalid/http:test",
        flavour: Flavour::Warm,
        run_id: "absent",
        timeout: Duration::from_secs(1),
        retries: 1,
        halt: &halt,
        journal: &journal,
    };
    let (result, latency) = starter.start(0).await;
    assert_eq!(
        category(&result),
        (
            "unanswered",
            "no answer after 2 attempts (0 timeouts, 2 no responders)"
        )
    );
    assert!(latency >= Duration::from_millis(600), "{latency:?}");
    assert!(halt.load(Ordering::Relaxed));
    assert!(matches!(starter.start(1).await.0, Started::Skipped));
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn measurements_record_serving_evidence_and_stop_for_failures() -> Result<()> {
    let fixture = Fixture::new().await?;
    for (flavour, status, reply, expected) in [
        (Flavour::Warm, 200, None, "cap"),
        (Flavour::ScaleToZero, 200, None, "cap"),
        (Flavour::Warm, 503, None, "warm-up"),
        (Flavour::ScaleToZero, 503, None, "sampled"),
        (
            Flavour::Warm,
            200,
            Some(Reply::State(WorkloadState::Error, "guest memory budget")),
            "refused",
        ),
        (Flavour::Warm, 200, Some(Reply::Bytes(b"invalid")), "failed"),
    ] {
        let http = HttpFixture::new(status).await?;
        let args = fixture.args(&http);
        fixture.replies("workload.start", reply.into_iter().collect());
        let cancel = tokio_util::sync::CancellationToken::new();
        let run = Run {
            args: &args,
            memory_limit: 1 << 40,
            run_id: "measurement",
            client: &fixture.client,
            subject: "test subject".into(),
            cancel: &cancel,
        };
        let ((report, stop), cleanup) =
            timeout(Duration::from_secs(10), run_flavour(&run, flavour)).await??;
        assert!(cleanup.is_none());
        match (expected, &stop) {
            ("cap", Stop::Cap { cap: 2 })
            | (
                "refused",
                Stop::StartRefused {
                    class: RefusalClass::MemoryLimit,
                    ..
                },
            )
            | ("failed", Stop::StartsFailed { .. }) => {}
            ("warm-up" | "sampled", Stop::Requests { stage, example, .. }) => {
                assert_eq!(*stage, expected);
                assert_eq!(example.as_deref(), Some("503 Service Unavailable: test"));
                assert!(!stop.to_string().contains("test"), "{stop}");
            }
            _ => panic!("{expected}: unexpected stop {stop:?}"),
        }
        assert_eq!(report.subject, "test subject");
        if expected == "cap" {
            assert_eq!(report.served(), 2);
            assert_eq!(report.steps.len(), 2);
            assert!(report.steps.iter().all(|s| s.clean));
            assert!(
                http.requests
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|r| r.contains("measurement.scale.test"))
            );
        } else {
            assert_eq!(report.served(), 0);
        }
    }
    let started = fixture.workload_ids("workload.start");
    let stopped = fixture.workload_ids("workload.stop");
    assert!(!started.is_empty());
    assert!(
        started
            .iter()
            .all(|id| id.starts_with("bench-scale-measurement-"))
    );
    assert!(
        started.is_subset(&stopped),
        "started {started:?}, stopped {stopped:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn unanswered_starts_stop_the_flavour() -> Result<()> {
    let fixture = Fixture::new().await?;
    let http = HttpFixture::new(200).await?;
    let args = fixture.args(&http);
    fixture.replies(
        "workload.start",
        vec![Reply::State(WorkloadState::Starting, "")],
    );
    fixture.replies(
        "workload.status",
        vec![Reply::State(WorkloadState::NotFound, "")],
    );
    let cancel = tokio_util::sync::CancellationToken::new();
    let run = Run {
        args: &args,
        memory_limit: 1 << 40,
        run_id: "unanswered",
        client: &fixture.client,
        subject: "test subject".into(),
        cancel: &cancel,
    };
    let ((report, stop), cleanup) =
        timeout(Duration::from_secs(10), run_flavour(&run, Flavour::Warm)).await??;
    assert!(cleanup.is_none());
    match &stop {
        Stop::StartsUnanswered { count, example } => {
            assert_eq!(*count, 1);
            assert_eq!(
                example,
                "no answer after 1 attempts (0 timeouts, 0 no responders)"
            );
        }
        other => panic!("unexpected stop {other:?}"),
    }
    assert_eq!(report.served(), 0);
    let id = workload_id("unanswered", Flavour::Warm, 0);
    assert_eq!(
        fixture.workload_ids("workload.start"),
        BTreeSet::from([id.clone()])
    );
    assert!(fixture.workload_ids("workload.stop").contains(&id));
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn main_run_writes_reports_and_distinguishes_insufficient_serving() -> Result<()> {
    let fixture = Fixture::new().await?;
    let http = HttpFixture::new(200).await?;
    let dir = tempfile::tempdir()?;
    let subject = format!(
        "example.invalid/http:test on wash contract-test ({})",
        fixture.id
    );
    for (flavour, min_components, exit) in [
        (crate::cli::FlavourArg::Both, 1, 0),
        (crate::cli::FlavourArg::Warm, 3, 2),
        (crate::cli::FlavourArg::ScaleToZero, 1, 0),
    ] {
        let mut args = fixture.args(&http);
        args.flavour = flavour;
        args.min_components = min_components;
        args.json = Some(dir.path().join(format!("{exit}/report.json")));
        assert_eq!(
            crate::run(args.clone(), &tokio_util::sync::CancellationToken::new()).await?,
            exit
        );
        let reports: Vec<Value> = serde_json::from_slice(&std::fs::read(args.json.unwrap())?)?;
        assert_eq!(
            reports.len(),
            if flavour == crate::cli::FlavourArg::Both {
                2
            } else {
                1
            }
        );
        assert!(reports.iter().all(|r| r["subject"] == subject));
    }
    fixture.replies("heartbeat", vec![Reply::Bytes(b"invalid")]);
    let error = crate::run(
        fixture.args(&http),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        error.starts_with(&format!("host {} did not answer a heartbeat", fixture.id)),
        "{error}"
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn measured_process_footprints_stop_at_the_memory_limit() -> Result<()> {
    let fixture = Fixture::new().await?;
    let http = HttpFixture::new(200).await?;
    let mut args = fixture.args(&http);
    args.host_pid = Some(std::process::id());
    let cancel = tokio_util::sync::CancellationToken::new();
    for (limit, flavour) in [(1, Flavour::Warm), (1 << 40, Flavour::ScaleToZero)] {
        let run = Run {
            args: &args,
            memory_limit: limit,
            run_id: "footprint",
            client: &fixture.client,
            subject: "process memory".into(),
            cancel: &cancel,
        };
        let ((report, stop), cleanup) =
            timeout(Duration::from_secs(10), run_flavour(&run, flavour)).await??;
        assert!(cleanup.is_none());
        assert!(
            report
                .baseline_footprint_bytes
                .is_some_and(|bytes| bytes > 0)
        );
        assert!(
            report
                .steps
                .iter()
                .all(|step| step.footprint_bytes.is_some())
        );
        if limit == 1 {
            assert!(matches!(stop, Stop::MemoryLimit { .. }));
        } else {
            assert!(matches!(stop, Stop::Cap { cap: 2 }));
            assert!(report.marginal_bytes_per_workload.is_some());
            assert!(report.limits.contains_key("memory limit"));
            assert!(
                report
                    .steps
                    .iter()
                    .all(|step| step.bytes_per_workload.is_some())
            );
        }
    }
    args.host_pid = Some(u32::MAX);
    args.max_components = None;
    let run = Run {
        args: &args,
        memory_limit: 1 << 40,
        run_id: "missing-process",
        client: &fixture.client,
        subject: "process memory".into(),
        cancel: &cancel,
    };
    let error = timeout(Duration::from_secs(10), run_flavour(&run, Flavour::Warm))
        .await?
        .unwrap_err();
    assert!(
        error.to_string().contains("footprint unavailable"),
        "{error:#}"
    );
    Ok(())
}
