//! The single long-lived **Service**. One instance, two roles:
//!
//!  * **HTTP ingress + router** (`wasi:http/handler`): the runtime routes
//!    inbound HTTP here because this is the service. It parses the request and
//!    dispatches by path to the stateless `users`/`todos` components over
//!    host-linked WIT calls.
//!  * **Shared Postgres session pool** (`wasi:cli/run`): it listens on a
//!    loopback Postgres port (`127.0.0.1:6432`) and hands each client
//!    connection a **pre-authenticated, pooled** session to the real upstream
//!    Postgres (session pooling, as in pgbouncer's session mode).
//!
//! This is the state/compute split that makes serverless database access
//! efficient: the stateless `users`/`todos` components open cheap loopback
//! connections (virtualized in-process — no TCP, no TLS, no auth) and may be
//! torn down at any time, while the expensive part — real TCP connections
//! authenticated against the database — lives here, bounded and reused across
//! both components and across instance churn. The database credentials exist
//! only in this component; the stateless components never see them.
//!
//! Lifecycle of one client connection:
//!
//!  1. The client (sqlx in `users`/`todos`) connects to `127.0.0.1:6432` and
//!     sends a Postgres `StartupMessage`. The pool answers the handshake
//!     itself — `AuthenticationOk` without credentials, since only components
//!     in this workload can reach the loopback — and replays the real server's
//!     `ParameterStatus` values so the client sees an ordinary Postgres.
//!  2. A pooled upstream session is checked out (or dialed and authenticated,
//!     up to `MAX_SESSIONS`; checkouts past the cap wait for a return).
//!  3. Bytes are spliced between client and session at message granularity.
//!  4. When the client disconnects — cleanly with `Terminate` or abruptly when
//!     a serverless instance is torn down — the session is reset with
//!     `Sync` + `ROLLBACK` + `DISCARD ALL` and returned to the pool for the
//!     next client. Sessions that fail to reset are closed, freeing capacity.
//!
//! Not supported (out of scope for a session-pooling template): `COPY`
//! sub-protocol, `CancelRequest`, and TLS on the loopback hop.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({
        world: "service",
        generate_all,
    });
}

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::task::{Poll, Waker};

use bindings::exports::wasi::cli::run::Guest as RunGuest;
use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Method, Request, Response};
use bindings::wasi::sockets::types::{
    IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
};
use bindings::wasmcloud::app::{todos, users};

/// Loopback Postgres endpoint the stateless backends connect to.
const POOLER_ADDR: Ipv4SocketAddress = Ipv4SocketAddress {
    port: 6432,
    address: (127, 0, 0, 1),
};

/// Upper bound on concurrent upstream sessions. Checkouts past the cap wait
/// for a session to be returned instead of dialing more.
const MAX_SESSIONS: usize = 4;

/// Sessions dialed and authenticated at startup, before the first request.
const PREWARM_SESSIONS: usize = 2;

/// Upper bound on messages drained during a session reset. A session that
/// exceeds it is protocol-wedged (e.g. dropped mid-`COPY`) and is closed
/// rather than reused.
const DRAIN_LIMIT: usize = 10_000;

/// Upper bound on a single protocol message from either peer. Postgres allows
/// up to 1 GiB; anything past this is treated as a corrupted length and the
/// connection is closed rather than buffered toward it.
const MAX_MESSAGE_LEN: i32 = 16 * 1024 * 1024;

struct Component;

// ---------------------------------------------------------------------------
// HTTP ingress + router (wasi:http/handler)
// ---------------------------------------------------------------------------

impl HttpGuest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let method = method_str(&request.get_method());
        let target = request
            .get_path_with_query()
            .unwrap_or_else(|| "/".to_string());
        let path = target.split('?').next().unwrap_or("/").to_string();

        let (status, content_type, body) = route(&method, &path).await;
        Ok(make_response(status, &content_type, body))
    }
}

/// Dispatch by path to the matching stateless backend over a host-linked WIT
/// call. The backends run sqlx on a current-thread Tokio runtime (the sqlx
/// `wasm32-wasip2` pattern), so this call is synchronous.
async fn route(method: &str, path: &str) -> (u16, String, Vec<u8>) {
    if path == "/users" || path.starts_with("/users/") {
        let r = users::handle(users::Request {
            method: method.to_string(),
            path: path.to_string(),
            query: String::new(),
            body: Vec::new(),
        })
        .await;
        (r.status, r.content_type, r.body)
    } else if path == "/todos" || path.starts_with("/todos/") {
        let r = todos::handle(todos::Request {
            method: method.to_string(),
            path: path.to_string(),
            query: String::new(),
            body: Vec::new(),
        })
        .await;
        (r.status, r.content_type, r.body)
    } else if path == "/" {
        (
            200,
            "application/json".to_string(),
            br#"{"routes":["/users","/todos"]}"#.to_vec(),
        )
    } else {
        (
            404,
            "application/json".to_string(),
            br#"{"error":"not found"}"#.to_vec(),
        )
    }
}

fn method_str(method: &Method) -> String {
    match method {
        Method::Get => "GET".to_string(),
        Method::Post => "POST".to_string(),
        Method::Put => "PUT".to_string(),
        Method::Delete => "DELETE".to_string(),
        Method::Patch => "PATCH".to_string(),
        Method::Head => "HEAD".to_string(),
        Method::Options => "OPTIONS".to_string(),
        Method::Other(s) => s.clone(),
        _ => "GET".to_string(),
    }
}

fn make_response(status: u16, content_type: &str, body: Vec<u8>) -> Response {
    let headers = Fields::new();
    let _ = headers.set("content-type", &[content_type.as_bytes().to_vec()]);
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    wit_bindgen::spawn_local(async move {
        tx.write_all(body).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    let _ = response.set_status_code(status);
    response
}

// ---------------------------------------------------------------------------
// Shared session pool (wasi:cli/run)
// ---------------------------------------------------------------------------

impl RunGuest for Component {
    async fn run() -> Result<(), ()> {
        let upstream = match upstream_config() {
            Some(u) => u,
            None => {
                eprintln!("service: UPSTREAM_ADDR is not a valid host:port");
                return Err(());
            }
        };

        let listener = create_listener(POOLER_ADDR).map_err(|e| {
            eprintln!("service: failed to bind {POOLER_ADDR:?}: {e}");
        })?;
        let mut accept = listener.listen().map_err(|e| {
            eprintln!("service: listen failed: {e:?}");
        })?;

        let pool = Rc::new(Pool::new(upstream));

        // Prewarm concurrently with accepting: readiness to serve is not
        // gated on the upstream dials (a slow or unreachable database should
        // not delay the listener), and clients that connect early simply
        // check out sessions on demand.
        let prewarm = async {
            pool.prewarm().await;
            // Log the identity in use (never the password) so a mis-set
            // UPSTREAM_USER/UPSTREAM_DB is visible instead of a silent
            // fallback to the defaults.
            eprintln!(
                "service: pool ready on 127.0.0.1:{} ({} pre-authenticated as {:?} to db {:?}, cap {})",
                POOLER_ADDR.port,
                pool.idle.borrow().len(),
                pool.upstream.user,
                pool.upstream.database,
                MAX_SESSIONS,
            );
        };
        let accept_loop = async {
            while let Some(client) = accept.next().await {
                wit_bindgen::spawn_local(serve_client(client, Rc::clone(&pool)));
            }
        };
        futures::join!(prewarm, accept_loop);
        Ok(())
    }
}

/// Upstream address and the credentials the pool authenticates with. Held
/// only by this component; clients on the loopback never present them.
struct Upstream {
    addr: Ipv4SocketAddress,
    user: String,
    password: String,
    database: String,
}

/// The shared pool: idle pre-authenticated sessions, a live count against
/// `MAX_SESSIONS`, and wakers for checkouts waiting at the cap. The guest is
/// single-threaded, so `RefCell`/`Cell` are all the synchronization needed.
struct Pool {
    upstream: Upstream,
    idle: RefCell<Vec<Session>>,
    /// Sessions currently open (idle + checked out).
    live: Cell<usize>,
    /// Checkouts blocked at the cap, woken on return or close.
    waiters: RefCell<Vec<Waker>>,
    /// Raw `ParameterStatus` payloads captured from the first upstream
    /// handshake, replayed verbatim to every loopback client so it sees the
    /// real server's settings (encoding, version, ...).
    server_params: RefCell<Vec<Vec<u8>>>,
    /// Source of fabricated `BackendKeyData` process ids for clients.
    next_client_id: Cell<i32>,
}

/// Outcome of the non-blocking half of a checkout.
enum Checkout {
    /// An idle session was available.
    Reuse(Session),
    /// A capacity slot was claimed; the caller dials a new session.
    Dial,
}

impl Pool {
    fn new(upstream: Upstream) -> Self {
        Pool {
            upstream,
            idle: RefCell::new(Vec::new()),
            live: Cell::new(0),
            waiters: RefCell::new(Vec::new()),
            server_params: RefCell::new(Vec::new()),
            next_client_id: Cell::new(1),
        }
    }

    /// Dial and authenticate the initial sessions so the first requests are
    /// served from warm connections (and so the server parameters are
    /// captured before the first client handshake).
    async fn prewarm(&self) {
        for _ in 0..PREWARM_SESSIONS {
            // Claim a capacity slot before dialing, exactly like a checkout:
            // clients may already be connecting concurrently, and the cap is
            // a shared budget — prewarm must not push the pool past it.
            if self.live.get() >= MAX_SESSIONS {
                return;
            }
            self.live.set(self.live.get() + 1);
            match self.dial().await {
                Ok(session) => {
                    self.idle.borrow_mut().push(session);
                    // A checkout may be waiting at the cap for this session.
                    self.wake_waiters();
                }
                Err(e) => {
                    self.live.set(self.live.get() - 1);
                    self.wake_waiters();
                    eprintln!("service: prewarm failed (will retry on demand): {e}");
                    return;
                }
            }
        }
    }

    /// Check out a session: reuse an idle one, dial a new one under the cap,
    /// or wait for a return.
    async fn acquire(self: &Rc<Self>) -> Result<Session, String> {
        let claimed = futures::future::poll_fn(|cx| {
            if let Some(session) = self.idle.borrow_mut().pop() {
                return Poll::Ready(Checkout::Reuse(session));
            }
            if self.live.get() < MAX_SESSIONS {
                self.live.set(self.live.get() + 1);
                return Poll::Ready(Checkout::Dial);
            }
            self.waiters.borrow_mut().push(cx.waker().clone());
            Poll::Pending
        })
        .await;

        match claimed {
            Checkout::Reuse(session) => Ok(session),
            Checkout::Dial => match self.dial().await {
                Ok(session) => Ok(session),
                Err(e) => {
                    // Release the claimed capacity slot.
                    self.live.set(self.live.get() - 1);
                    self.wake_waiters();
                    Err(e)
                }
            },
        }
    }

    /// Return a healthy (reset) session for the next client.
    fn release(&self, session: Session) {
        self.idle.borrow_mut().push(session);
        self.wake_waiters();
    }

    /// Discard a broken session, freeing its capacity slot.
    fn close(&self, session: Session) {
        drop(session);
        self.live.set(self.live.get() - 1);
        self.wake_waiters();
    }

    fn wake_waiters(&self) {
        for waker in self.waiters.borrow_mut().drain(..) {
            waker.wake();
        }
    }

    /// Dial the upstream and run the Postgres startup + authentication
    /// handshake with the pool's credentials.
    async fn dial(&self) -> Result<Session, String> {
        let mut session = Session::connect(IpSocketAddress::Ipv4(self.upstream.addr))
            .await
            .ok_or_else(|| "upstream connect failed".to_string())?;

        session
            .tx
            .write_all(startup_message(
                &self.upstream.user,
                &self.upstream.database,
            ))
            .await;

        let capture = self.server_params.borrow().is_empty();
        loop {
            let msg = match session.read_msg().await {
                Some(msg) => msg,
                None => return Err("upstream closed during handshake".to_string()),
            };
            match msg.kind {
                // AuthenticationRequest; the payload's leading i32 says which.
                b'R' => match be_i32(msg.payload()) {
                    Some(0) => {} // AuthenticationOk
                    Some(3) => {
                        // AuthenticationCleartextPassword
                        let mut password = self.upstream.password.clone().into_bytes();
                        password.push(0);
                        session.tx.write_all(pg_msg(b'p', &password)).await;
                    }
                    Some(code) => {
                        return Err(format!(
                            "unsupported auth method {code} (this template supports \
                             trust and password; set POSTGRES_HOST_AUTH_METHOD=password)"
                        ));
                    }
                    None => return Err("malformed authentication message".to_string()),
                },
                // ParameterStatus: capture once, replayed to every client.
                b'S' => {
                    if capture {
                        self.server_params.borrow_mut().push(msg.payload().to_vec());
                    }
                }
                b'K' => {} // BackendKeyData: clients get fabricated ones.
                b'E' => return Err(error_response_text(msg.payload())),
                b'N' => {} // NoticeResponse
                // ReadyForQuery: the session is authenticated and idle.
                b'Z' => return Ok(session),
                _ => {}
            }
        }
    }
}

/// A pooled upstream session: a live socket plus its stream halves. Outbound
/// bytes are written to `tx`; inbound messages are read via `read_msg`
/// (buffered in `buf`).
struct Session {
    _sock: Rc<TcpSocket>,
    tx: ByteWriter,
    rx: ByteReader,
    // The receive-completion future must stay alive for the life of the
    // session: dropping it cancels the receive, which would make the next
    // (reused) read on a pooled session fail.
    _recv: RecvFuture,
    buf: Vec<u8>,
}

/// Byte stream halves produced by `wit_stream::new()` and `receive()`.
type ByteWriter = wit_bindgen::StreamWriter<u8>;
type ByteReader = wit_bindgen::StreamReader<u8>;
/// The `future<result<_, error-code>>` half returned by `tcp-socket.receive`.
type RecvFuture = wit_bindgen::FutureReader<Result<(), bindings::wasi::sockets::types::ErrorCode>>;

impl Session {
    async fn connect(addr: IpSocketAddress) -> Option<Session> {
        let sock = Rc::new(TcpSocket::create(IpAddressFamily::Ipv4).ok()?);
        sock.connect(addr).await.ok()?;
        Some(Session::from_socket(sock))
    }

    /// Wrap an already-connected socket (also used for accepted clients).
    fn from_socket(sock: Rc<TcpSocket>) -> Session {
        let (tx, out_rx) = bindings::wit_stream::new();
        let send_sock = Rc::clone(&sock);
        // Drive the outbound stream for the life of the session.
        wit_bindgen::spawn_local(async move {
            let _ = send_sock.send(out_rx).await;
        });
        let (rx, recv) = sock.receive();
        Session {
            _sock: sock,
            tx,
            rx,
            _recv: recv,
            buf: Vec::new(),
        }
    }

    /// Read one regular Postgres message (type byte + length-prefixed body).
    /// Returns `None` when the peer closes.
    async fn read_msg(&mut self) -> Option<Msg> {
        read_pg_msg(&mut self.rx, &mut self.buf).await
    }
}

/// One regular Postgres protocol message, kept in wire form so it can be
/// forwarded without re-encoding. `raw` is `kind` + i32 length + body.
struct Msg {
    kind: u8,
    raw: Vec<u8>,
}

impl Msg {
    /// The message body (after the type byte and length).
    fn payload(&self) -> &[u8] {
        &self.raw[5..]
    }
}

// ---------------------------------------------------------------------------
// Per-client serving: handshake, splice, reset, return.
// ---------------------------------------------------------------------------

/// How the client side of a splice ended.
#[derive(Clone, Copy, PartialEq)]
enum ClientEnd {
    /// Still connected.
    Open,
    /// Sent `Terminate` or dropped; a reset is in flight on the session.
    Resetting,
}

async fn serve_client(client_sock: TcpSocket, pool: Rc<Pool>) {
    let mut client = Session::from_socket(Rc::new(client_sock));

    // -- Handshake: answer the startup ourselves; no credentials required on
    // the loopback (only components in this workload can reach it).
    if !client_handshake(&mut client, &pool).await {
        return;
    }

    // -- Check out a pooled upstream session (may wait at the cap).
    let session = match pool.acquire().await {
        Ok(session) => session,
        Err(e) => {
            eprintln!("service: checkout failed: {e}");
            // The handshake already completed, so dropping the client socket
            // surfaces in sqlx as an unexpected close on its first query.
            return;
        }
    };

    // -- Splice until the client goes away, then reset the session. The two
    // pumps run concurrently in this task, so each connection is split into
    // its disjoint halves (reads on one side, writes on the other).
    let end = Cell::new(ClientEnd::Open);
    let reusable = Cell::new(false);
    let Session {
        _sock: client_sock,
        tx: client_tx,
        rx: mut client_rx,
        _recv: client_recv,
        buf: mut client_buf,
    } = client;
    let Session {
        _sock: session_sock,
        tx: mut session_tx,
        rx: mut session_rx,
        _recv: session_recv,
        buf: mut session_buf,
    } = session;

    // Client -> session: forward messages, intercepting `Terminate`. On any
    // client end (graceful or an abrupt instance teardown) start the reset:
    // `Sync` closes any open extended-protocol sequence, `ROLLBACK` clears
    // any open transaction, `DISCARD ALL` clears session state (prepared
    // statements, GUCs, temp tables). The reset traffic also wakes the
    // session-side pump so it can observe the state change.
    //
    // Writes to a peer that has meanwhile gone away are safe: a wit stream
    // write to a dropped reader returns promptly with the unwritten values
    // (`StreamResult::Dropped`) rather than blocking.
    let client_to_session = async {
        loop {
            match read_pg_msg(&mut client_rx, &mut client_buf).await {
                Some(msg) if msg.kind == b'X' => break, // Terminate: not forwarded
                Some(msg) => {
                    session_tx.write_all(msg.raw).await;
                }
                None => break, // client dropped without Terminate
            }
        }
        end.set(ClientEnd::Resetting);
        session_tx.write_all(pg_msg(b'S', &[])).await; // Sync
        session_tx.write_all(query_message("ROLLBACK")).await;
        session_tx.write_all(query_message("DISCARD ALL")).await;
    };

    // Session -> client: forward messages while the client is connected; once
    // the reset starts, discard responses until `CommandComplete` for
    // `DISCARD ALL` followed by `ReadyForQuery` — the session is then clean
    // and reusable. `DRAIN_LIMIT` bounds a protocol-wedged session.
    let session_to_client = async {
        // Own the client's socket and write half so they drop the moment this
        // pump exits. When the upstream dies mid-session this is what unwinds
        // everything: closing the client's socket ends the other pump's read
        // and surfaces an error to the client instead of an indefinite hang —
        // and the freed pool slot lets the client's retry dial a fresh
        // session.
        let _client_sock = client_sock;
        let mut client_tx = client_tx;
        let mut drained = 0usize;
        let mut discard_done = false;
        loop {
            let msg = match read_pg_msg(&mut session_rx, &mut session_buf).await {
                Some(msg) => msg,
                None => break, // upstream closed: not reusable
            };
            match end.get() {
                ClientEnd::Open => {
                    client_tx.write_all(msg.raw).await;
                }
                ClientEnd::Resetting => {
                    drained += 1;
                    if drained > DRAIN_LIMIT {
                        eprintln!("service: session reset overran; closing session");
                        break;
                    }
                    if msg.kind == b'C' && msg.payload().starts_with(b"DISCARD ALL") {
                        discard_done = true;
                    } else if msg.kind == b'Z' && discard_done {
                        reusable.set(true);
                        break;
                    }
                }
            }
        }
    };

    futures::join!(client_to_session, session_to_client);
    drop((client_rx, client_recv, client_buf));

    let session = Session {
        _sock: session_sock,
        tx: session_tx,
        rx: session_rx,
        _recv: session_recv,
        buf: session_buf,
    };
    if reusable.get() {
        pool.release(session);
    } else {
        pool.close(session);
    }
}

/// Run the server side of the Postgres startup handshake with a loopback
/// client: `AuthenticationOk` (no credentials on the loopback), the captured
/// upstream `ParameterStatus` values, a fabricated `BackendKeyData`, and
/// `ReadyForQuery`. Returns `false` if the client vanishes or speaks
/// something other than protocol 3.0.
async fn client_handshake(client: &mut Session, pool: &Pool) -> bool {
    const PROTOCOL_3_0: i32 = 196608;
    const SSL_REQUEST: i32 = 80877103;
    const GSSENC_REQUEST: i32 = 80877104;

    loop {
        let payload = match read_startup(&mut client.rx, &mut client.buf).await {
            Some(payload) => payload,
            None => return false,
        };
        match be_i32(&payload) {
            // TLS/GSS on the in-process loopback would be pure overhead;
            // decline and the client continues in the clear.
            Some(SSL_REQUEST) | Some(GSSENC_REQUEST) => {
                client.tx.write_all(vec![b'N']).await;
            }
            Some(PROTOCOL_3_0) => break,
            _ => return false, // CancelRequest or unknown: unsupported
        }
    }

    let mut reply = pg_msg(b'R', &0i32.to_be_bytes()); // AuthenticationOk
    for params in pool.server_params.borrow().iter() {
        reply.extend_from_slice(&pg_msg(b'S', params));
    }
    let id = pool.next_client_id.get();
    pool.next_client_id.set(id.wrapping_add(1));
    let mut key_data = Vec::with_capacity(8);
    key_data.extend_from_slice(&id.to_be_bytes());
    key_data.extend_from_slice(&(!id).to_be_bytes());
    reply.extend_from_slice(&pg_msg(b'K', &key_data)); // BackendKeyData
    reply.extend_from_slice(&pg_msg(b'Z', b"I")); // ReadyForQuery (idle)
    client.tx.write_all(reply).await;
    true
}

// ---------------------------------------------------------------------------
// Postgres wire encoding/decoding.
// ---------------------------------------------------------------------------

/// Encode a regular message: type byte, then an i32 length that counts itself
/// plus the body.
fn pg_msg(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(5 + body.len());
    msg.push(kind);
    msg.extend_from_slice(&((4 + body.len()) as i32).to_be_bytes());
    msg.extend_from_slice(body);
    msg
}

/// Encode a simple `Query` message.
fn query_message(sql: &str) -> Vec<u8> {
    let mut body = sql.as_bytes().to_vec();
    body.push(0);
    pg_msg(b'Q', &body)
}

/// Encode the `StartupMessage` (no type byte; length counts itself).
fn startup_message(user: &str, database: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&196608i32.to_be_bytes()); // protocol 3.0
    for (key, value) in [("user", user), ("database", database)] {
        body.extend_from_slice(key.as_bytes());
        body.push(0);
        body.extend_from_slice(value.as_bytes());
        body.push(0);
    }
    body.push(0);
    let mut msg = Vec::with_capacity(4 + body.len());
    msg.extend_from_slice(&((4 + body.len()) as i32).to_be_bytes());
    msg.extend_from_slice(&body);
    msg
}

/// Read one regular message (type byte + i32 length + body) in wire form.
async fn read_pg_msg(rx: &mut ByteReader, buf: &mut Vec<u8>) -> Option<Msg> {
    while buf.len() < 5 {
        if !fill(rx, buf).await {
            return None;
        }
    }
    let len = be_i32(&buf[1..5])?; // counts itself, not the type byte
    if !(4..=MAX_MESSAGE_LEN).contains(&len) {
        return None; // malformed or corrupted length
    }
    let total = 1 + len as usize;
    while buf.len() < total {
        if !fill(rx, buf).await {
            return None;
        }
    }
    let raw: Vec<u8> = buf.drain(..total).collect();
    Some(Msg { kind: raw[0], raw })
}

/// Read one startup-style message (i32 length + body, no type byte),
/// returning just the body.
async fn read_startup(rx: &mut ByteReader, buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    while buf.len() < 4 {
        if !fill(rx, buf).await {
            return None;
        }
    }
    let len = be_i32(&buf[..4])?; // counts itself
    if !(4..=MAX_MESSAGE_LEN).contains(&len) {
        return None; // malformed or corrupted length
    }
    let len = len as usize;
    while buf.len() < len {
        if !fill(rx, buf).await {
            return None;
        }
    }
    let mut raw: Vec<u8> = buf.drain(..len).collect();
    raw.drain(..4);
    Some(raw)
}

/// Pull the next chunk off the stream into `buf`. Returns `false` when the
/// peer has closed and no bytes arrived.
async fn fill(rx: &mut ByteReader, buf: &mut Vec<u8>) -> bool {
    use wit_bindgen::StreamResult;
    let (result, chunk) = rx.read(Vec::with_capacity(8192)).await;
    let got = !chunk.is_empty();
    buf.extend_from_slice(&chunk);
    got || !matches!(result, StreamResult::Dropped)
}

fn be_i32(bytes: &[u8]) -> Option<i32> {
    Some(i32::from_be_bytes(bytes.get(..4)?.try_into().ok()?))
}

/// Extract the human-readable message from an `ErrorResponse` payload
/// (severity/code/message fields as `type-byte + cstring` pairs).
fn error_response_text(payload: &[u8]) -> String {
    let mut rest = payload;
    while let [kind, tail @ ..] = rest {
        if *kind == 0 {
            break;
        }
        let end = tail.iter().position(|b| *b == 0).unwrap_or(tail.len());
        if *kind == b'M' {
            return String::from_utf8_lossy(&tail[..end]).to_string();
        }
        // A field without a NUL terminator is malformed; stop rather than
        // slicing past the end.
        rest = tail.get(end + 1..).unwrap_or(&[]);
    }
    "upstream error".to_string()
}

// ---------------------------------------------------------------------------
// Plumbing.
// ---------------------------------------------------------------------------

fn create_listener(addr: Ipv4SocketAddress) -> Result<TcpSocket, String> {
    let sock = TcpSocket::create(IpAddressFamily::Ipv4).map_err(|e| format!("create: {e:?}"))?;
    sock.bind(IpSocketAddress::Ipv4(addr))
        .map_err(|e| format!("bind: {e:?}"))?;
    let _ = sock.set_listen_backlog_size(128);
    Ok(sock)
}

/// Read the upstream address and credentials from workload config. The
/// credentials live only in this component.
fn upstream_config() -> Option<Upstream> {
    let raw = std::env::var("UPSTREAM_ADDR").ok()?;
    let (host, port) = raw.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let mut octets = [0u8; 4];
    let mut parts = host.split('.');
    for o in octets.iter_mut() {
        *o = parts.next()?.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| "app".to_string());
    Some(Upstream {
        addr: Ipv4SocketAddress {
            port,
            address: (octets[0], octets[1], octets[2], octets[3]),
        },
        user: var("UPSTREAM_USER"),
        password: var("UPSTREAM_PASSWORD"),
        database: var("UPSTREAM_DB"),
    })
}

mod export {
    #![allow(unsafe_code)]
    use super::{bindings, Component};
    bindings::export!(Component with_types_in bindings);
}
