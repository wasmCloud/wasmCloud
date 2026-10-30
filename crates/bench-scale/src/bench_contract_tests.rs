use super::*;
use std::collections::{HashMap, VecDeque};
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
    replies: HashMap<&'static str, VecDeque<Reply>>,
    requests: Vec<(String, Value)>,
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
        let (url, server) = if let Ok(url) = std::env::var("WASH_TEST_NATS_URL") {
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
                    let bytes = match reply {
                        Some(Reply::Silent) => continue,
                        Some(Reply::Bytes(bytes)) => bytes.to_vec(),
                        reply => {
                            let value = if command == "heartbeat" {
                                json!({"id": id, "version": "contract-test"})
                            } else {
                                let (state, message) = match reply {
                                    Some(Reply::State(state, message)) => (state, message),
                                    _ if command == "workload.start" => {
                                        (WorkloadState::Running, "")
                                    }
                                    _ if command == "workload.stop" => {
                                        (WorkloadState::Completed, "")
                                    }
                                    _ => (WorkloadState::NotFound, ""),
                                };
                                json!({"workloadStatus": {"workloadState": state.as_str_name(), "message": message}})
                            };
                            serde_json::to_vec(&value).unwrap()
                        }
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
                    let mut head = Vec::new();
                    while !head.ends_with(b"\r\n\r\n") {
                        head.push(socket.read_u8().await.unwrap());
                        assert!(head.len() < 8192);
                    }
                    requests
                        .lock()
                        .unwrap()
                        .push(String::from_utf8(head).unwrap());
                    socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntest").as_bytes()).await.unwrap();
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

#[tokio::test]
#[ignore = "requires NATS (WASH_TEST_NATS_URL or Docker)"]
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
#[ignore = "requires NATS (WASH_TEST_NATS_URL or Docker)"]
async fn starts_settle_retries_and_preserve_failure_categories() -> Result<()> {
    let fixture = Fixture::new().await?;
    for (starts, statuses, category) in [
        (
            vec![Reply::State(WorkloadState::Running, "")],
            vec![],
            "running",
        ),
        (
            vec![Reply::State(WorkloadState::Starting, "")],
            vec![Reply::State(WorkloadState::Running, "")],
            "running",
        ),
        (
            vec![Reply::Silent],
            vec![Reply::State(WorkloadState::Running, "")],
            "running",
        ),
        (
            vec![Reply::State(WorkloadState::Error, "memory limit")],
            vec![],
            "refused",
        ),
        (vec![Reply::Bytes(b"invalid")], vec![], "failed"),
        (
            vec![Reply::State(WorkloadState::Stopping, "stopping")],
            vec![],
            "failed",
        ),
        (
            vec![Reply::State(WorkloadState::Starting, "")],
            vec![Reply::State(WorkloadState::NotFound, "")],
            "unanswered",
        ),
        (
            vec![
                Reply::Silent,
                Reply::State(WorkloadState::Error, "duplicate"),
            ],
            vec![
                Reply::State(WorkloadState::NotFound, ""),
                Reply::State(WorkloadState::Running, ""),
            ],
            "running",
        ),
    ] {
        fixture.replies("workload.start", starts);
        fixture.replies("workload.status", statuses);
        let halt = AtomicBool::new(false);
        let journal = Journal::default();
        let starter = Starter {
            client: &fixture.client,
            image: "example.invalid/http:test",
            flavour: Flavour::Warm,
            run_id: "retry",
            timeout: Duration::from_millis(30),
            retries: u32::from(category == "running"),
            halt: &halt,
            journal: &journal,
        };
        let (result, _) = starter.start(0).await;
        assert!(
            matches!(
                (category, &result),
                ("running", Started::Running)
                    | ("refused", Started::Refused(_))
                    | ("failed", Started::Failed(_))
                    | ("unanswered", Started::Unanswered(_))
            ),
            "unexpected category {category}"
        );
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
#[ignore = "requires NATS (WASH_TEST_NATS_URL or Docker)"]
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
        assert!(matches!(
            (expected, stop),
            ("cap", Stop::Cap { cap: 2 })
                | (
                    "warm-up",
                    Stop::Requests {
                        stage: "warm-up",
                        ..
                    }
                )
                | (
                    "sampled",
                    Stop::Requests {
                        stage: "sampled",
                        ..
                    }
                )
                | (
                    "refused",
                    Stop::StartRefused {
                        class: RefusalClass::MemoryLimit,
                        ..
                    }
                )
                | ("failed", Stop::StartsFailed { .. })
        ));
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
    let script = fixture.script.lock().unwrap();
    let start = script
        .requests
        .iter()
        .find(|(cmd, _)| cmd == "workload.start")
        .unwrap();
    assert!(
        start
            .1
            .get("workloadId")
            .unwrap()
            .as_str()
            .unwrap()
            .starts_with("bench-scale-measurement-")
    );
    assert!(
        script
            .requests
            .iter()
            .any(|(cmd, _)| cmd == "workload.stop")
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires NATS (WASH_TEST_NATS_URL or Docker)"]
async fn main_run_writes_reports_and_distinguishes_insufficient_serving() -> Result<()> {
    let fixture = Fixture::new().await?;
    let http = HttpFixture::new(200).await?;
    let dir = tempfile::tempdir()?;
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
        assert!(reports.iter().all(|r| {
            r.get("subject")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("contract-test")
        }));
    }
    fixture.replies("heartbeat", vec![Reply::Bytes(b"invalid")]);
    assert!(
        crate::run(
            fixture.args(&http),
            &tokio_util::sync::CancellationToken::new()
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("heartbeat")
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
#[ignore = "requires NATS (WASH_TEST_NATS_URL or Docker)"]
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
