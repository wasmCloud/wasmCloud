use crate::server::TelnetServer;
use crate::{SessionStarted, TelnetMessage, OP_RECEIVE_TEXT, OP_SESSION_STARTED};
use ansi_escapes::*;
use crossbeam::channel::{Receiver, Sender};
use crossbeam_channel::{select, unbounded};
use log::info;
use std::{
    collections::HashMap,
    io::Write,
    net::*,
    sync::{Arc, RwLock},
};
use uuid::Uuid;
use wasmcloud_provider_core::{capabilities::Dispatcher, serialize};

/// Code that performs initial text sending to a newly connected socket (e.g. motd)
/// and then starts a read loop that takes characters from the client and adds
/// them to a buffer, which is then delivered to the actor upon encountering a carriage return
fn read_session(socket: TcpStream, sender: Sender<String>, motd: String) {
    let mut conn = TelnetServer::connect(socket, None);
    let motd = motd.replace('\n', "\n\r");
    match conn.negotiate_winsize() {
        Ok(true) => (),
        Ok(false) => eprintln!("no winsize"),
        Err(e) => eprintln!("no winsize: {}", e),
    }
    let termok = conn
        .negotiate_cbreak()
        .and_then(|_| conn.negotiate_noecho())
        .and_then(|_| conn.negotiate_ansi());
    match termok {
        Ok(true) => (),
        e => {
            let mut socket = conn.into_inner();
            eprintln!("cannot set up terminal: {:?}", e);
            socket
                .write(
                    b"Your telnet client cannot be put in no-echo single-character ANSI mode as needed by this server.\r\n",
                )
                .unwrap();
            return;
        }
    }
    conn.set_timeout(Some(100));
    //let width = conn.width.unwrap();
    //let height = conn.height.unwrap();

    macro_rules! cprint {
        ($fmt:expr, $($arg:expr),+) => {
            conn.write_all(format!($fmt, $($arg),*).as_bytes()).unwrap();
        };
    }

    cprint!("{}", motd);
    let mut buf: Vec<char> = Vec::new();
    loop {
        if let Ok(Some(s)) = conn.read() {
            let chars = s.chars().collect::<Vec<_>>();
            let val: u32 = chars[0] as u32;
            if val == 13 {
                sender.send(buf.iter().collect()).unwrap();
                buf.clear();
                cprint!("{}", "\r\n");
            } else if val == 0 {
                // ignore
            } else if val == 27 || val == 127 {
                // delete or backspace
                if !buf.is_empty() {
                    buf.remove(buf.len() - 1);
                }
                cprint!("{} {}", CursorMove::X(-1), CursorMove::X(-1));
            } else {
                cprint!("{}", s);
                buf.push(chars[0]);
            }
        }
    }
}

pub fn start_server(
    motd: String,
    port: u32,
    actor: &str,
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    outbounds: Arc<RwLock<HashMap<String, Sender<String>>>>,
) {
    info!(
        "Starting telnet session on port {} for actor {}",
        port, actor
    );
    let a = actor.to_string();

    std::thread::spawn(move || {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).unwrap();
        loop {
            let d = dispatcher.clone();
            let a = a.clone();
            let motd = motd.clone();
            let (socket, _) = listener.accept().unwrap();
            let session_id = Uuid::new_v4();
            let mut s = socket.try_clone().unwrap();

            let (reader_s, reader_r) = unbounded();
            let (writer_s, writer_r): (Sender<String>, Receiver<String>) = unbounded();
            outbounds
                .write()
                .unwrap()
                .insert(session_id.to_string(), writer_s);
            let sess_start = SessionStarted {
                session: session_id.to_string(),
            };

            let _ = std::thread::spawn(move || {
                read_session(socket.try_clone().unwrap(), reader_s, motd);
            });
            d.read()
                .unwrap()
                .dispatch(&a, OP_SESSION_STARTED, &serialize(sess_start).unwrap())
                .unwrap();
            std::thread::spawn(move || loop {
                select! {
                    recv(reader_r) -> msg => {
                        let tmsg = TelnetMessage{
                            session: session_id.to_string(),
                            text: msg.unwrap(),
                        };
                        d.read().unwrap().dispatch(&a, OP_RECEIVE_TEXT,
                                        &serialize(tmsg).unwrap()).unwrap();
                    },
                    recv(writer_r) -> msg => { s.write_all(msg.unwrap().as_bytes()).unwrap(); },
                }
            });
        }
    });
}
