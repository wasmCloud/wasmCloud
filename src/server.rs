use telnet::*;
use NegotiationAction::*;
use TelnetOption::*;

use core::time::*;
use std::io::{self, *};
use std::net::*;

// NOTE: much of this code for terminal negotiation and the like was copied from https://github.com/pdx-cs-rust/netdoor/blob/master/src/lib.rs

// Terminal type information from
// https://code.google.com/archive/p/bogboa/wikis/TerminalTypes.wiki
const TTYPES: &[&str] = &[
    "ansi",
    "xterm",
    "eterm",
    "rxvt",
    "tintin++",
    "gosclient",
    "mushclient",
    "zmud",
    "gosclient",
    "vt1",
    "tinyfugue",
];

// TTYPE subnegotiation commands.
const SEND: u8 = 1;
const IS: u8 = 0;

#[derive(Debug)]
pub struct NegotiationError(TelnetEvent);

impl std::fmt::Display for NegotiationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "negotiation error: {:?}", self.0)
    }
}

impl std::error::Error for NegotiationError {}

#[derive(Debug)]
pub struct TelnetError(String);

impl std::fmt::Display for TelnetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Telnet Error: {}", self.0)
    }
}

impl std::error::Error for TelnetError {}

pub struct TelnetServer {
    telnet: Telnet,
    ttype: Option<String>,
    next_event: Option<TelnetEvent>,
    timeout: Option<Duration>,
    pub cbreak: bool,
    pub echo: bool,
    pub ansi: bool,
    pub width: Option<u16>,
    pub height: Option<u16>,
}

impl TelnetServer {
    pub fn connect<B: Into<Option<usize>>>(stream: TcpStream, nbuf: B) -> TelnetServer {
        let nbuf = nbuf.into().unwrap_or(256);
        let telnet = Telnet::from_stream(Box::new(stream), nbuf);
        TelnetServer {
            telnet,
            ttype: None,
            next_event: None,
            timeout: None,
            cbreak: false,
            echo: true,
            ansi: false,
            width: None,
            height: None,
        }
    }

    pub fn negotiate_cbreak(&mut self) -> io::Result<bool> {
        self.telnet.negotiate(Will, SuppressGoAhead);
        let event = self.get_event()?;
        use TelnetEvent::*;
        match event {
            Negotiation(Do, SuppressGoAhead) => {
                self.cbreak = true;
                Ok(true)
            }
            Negotiation(Dont, SuppressGoAhead) => {
                self.cbreak = false;
                Ok(false)
            }
            event => {
                self.next_event = Some(event);
                Ok(false)
            }
        }
    }

    pub fn negotiate_noecho(&mut self) -> io::Result<bool> {
        // *We* will echo, so terminal should not.
        self.telnet.negotiate(Will, Echo);
        let event = self.get_event()?;
        use TelnetEvent::*;
        match event {
            Negotiation(Do, Echo) => {
                self.echo = false;
                Ok(true)
            }
            Negotiation(Dont, Echo) => {
                self.echo = true;
                Ok(false)
            }
            event => {
                self.next_event = Some(event);
                Ok(false)
            }
        }
    }

    pub fn negotiate_ansi(&mut self) -> io::Result<bool> {
        self.telnet.negotiate(Do, TTYPE);
        loop {
            let event = self.get_event()?;
            use TelnetEvent::*;
            match event {
                Negotiation(Will, TTYPE) => {
                    self.telnet.subnegotiate(TelnetOption::TTYPE, &[SEND]);
                }
                Negotiation(Wont, TTYPE) => {
                    self.ansi = false;
                    return Ok(false);
                }
                Subnegotiation(TTYPE, buf) => {
                    assert_eq!(buf[0], IS);
                    let ttype = std::str::from_utf8(&buf[1..]).unwrap().to_string();
                    for good_ttype in TTYPES {
                        let lc_ttype = ttype.to_lowercase();
                        if lc_ttype.starts_with(*good_ttype) {
                            self.ansi = true;
                            self.ttype = Some(ttype);
                            return Ok(true);
                        }
                    }
                    match self.ttype {
                        None => self.ttype = Some(ttype),
                        Some(ref st) if st == &ttype => {
                            self.ansi = false;
                            self.ttype = None;
                            return Ok(false);
                        }
                        _ => (),
                    }
                    self.telnet.subnegotiate(TTYPE, &[SEND]);
                }
                event => {
                    self.next_event = Some(event);
                    return Ok(false);
                }
            }
        }
    }

    pub fn negotiate_winsize(&mut self) -> io::Result<bool> {
        self.telnet.negotiate(Do, NAWS);
        loop {
            let event = self.get_event()?;
            use TelnetEvent::*;
            match event {
                Negotiation(Will, NAWS) => {
                    self.telnet.subnegotiate(TelnetOption::NAWS, &[]);
                }
                Negotiation(Wont, NAWS) => {
                    self.width = None;
                    self.height = None;
                    return Ok(false);
                }
                Subnegotiation(NAWS, buf) => {
                    assert_eq!(buf.len(), 4);
                    #[allow(clippy::cast_lossless)]
                    let width: u16 = (buf[0] as u16) << 8 | buf[1] as u16;
                    #[allow(clippy::cast_lossless)]
                    let height: u16 = (buf[2] as u16) << 8 | buf[3] as u16;
                    if width > 0 {
                        self.width = Some(width);
                    }
                    if height > 0 {
                        self.height = Some(height);
                    }
                    return Ok(width > 0 || height > 0);
                }
                event => {
                    self.next_event = Some(event);
                    return Ok(false);
                }
            }
        }
    }

    pub fn set_timeout(&mut self, ms: Option<u64>) {
        self.timeout = ms.map(Duration::from_millis);
    }

    fn get_event(&mut self) -> io::Result<TelnetEvent> {
        if let Some(event) = self.next_event.take() {
            return Ok(event);
        }
        match self.timeout {
            None => self.telnet.read(),
            Some(timeout) => self.telnet.read_timeout(timeout),
        }
    }

    pub fn read(&mut self) -> io::Result<Option<String>> {
        loop {
            let event = self.get_event()?;
            use TelnetEvent::*;
            match event {
                Data(buf) => match String::from_utf8(buf.to_vec()) {
                    Ok(s) => return Ok(Some(s)),
                    Err(e) => {
                        return Err(io::Error::new(ErrorKind::InvalidData, e));
                    }
                },
                TimedOut => return Ok(None),
                NoData => (),
                Error(msg) => {
                    return Err(io::Error::new(ErrorKind::InvalidData, TelnetError(msg)));
                }
                _ => {
                    return Err(io::Error::new(ErrorKind::Other, NegotiationError(event)));
                }
            }
        }
    }

    pub fn into_inner(self) -> Telnet {
        self.telnet
    }
}

impl Write for TelnetServer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.telnet.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
