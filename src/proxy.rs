mod forward;

use std::{
    array, fmt, io,
    net::{self, ToSocketAddrs},
    ops::ControlFlow,
};

use crate::{
    network::Addr,
    proxy::forward::Forwarder,
    reactor::{Reactor, Registered},
};

use mio::{
    event::{Event, Source},
    net::{TcpListener, TcpStream},
    Events, Interest, Poll, Token,
};
use slab::Slab;
use thiserror::Error as ThisError;

#[derive(Debug, ThisError)]
pub enum Error {
    #[error("Could not create mio poller: {0}")]
    CreatePoll(io::Error),

    #[error("Could not listen on {0}: {1}")]
    StartListening(String, io::Error),

    #[error("Could not poll: {0}")]
    Poll(io::Error),

    #[error("accept() failed on {0}: {1}")]
    Accept(String, io::Error),

    #[error("connect to {0} failed: {1}")]
    Connect(String, io::Error),

    #[error("forward: {0}")]
    Forward(io::Error),

    #[error("could not {0} OOB: {1}")]
    Oob(&'static str, io::Error),

    #[error("Unix domain sockets are not supported yet")]
    UnixDomain,

    #[error("{0}")]
    Other(String),
}

type Result<T> = std::result::Result<T, Error>;

pub struct Proxy {
    listen_addr: Addr,
    forward_addr: Addr,
    poll: Poll,
    token_counter: usize,
    forward_token_start: usize,
    listeners: Vec<(String, TcpListener)>,
    forwarders: Slab<Forwarder>,
}

impl Proxy {
    const TOKEN_BLOCK_SIZE: usize = 10;

    pub fn new(listen_addr: Addr, forward_addr: Addr) -> Result<Proxy> {
        let poll = Poll::new().map_err(Error::CreatePoll)?;
        let mut proxy = Proxy {
            listen_addr,
            forward_addr,
            poll,
            token_counter: 0,
            forward_token_start: usize::MAX,
            listeners: Default::default(),
            forwarders: Default::default(),
        };

        proxy.add_listeners()?;
        Ok(proxy)
    }

    fn add_listeners(&mut self) -> Result<()> {
        let Addr::Tcp(target) = &self.listen_addr else {
            return Err(Error::UnixDomain);
        };
        let addrs = target
            .to_socket_addrs()
            .map_err(|e| Error::StartListening(target.clone(), e))?;

        for addr in addrs {
            self.add_tcp_listener(addr)?;
        }

        let n = self.listeners.len();
        self.token_counter = n;
        self.forward_token_start = n;
        Ok(())
    }

    fn add_tcp_listener(&mut self, addr: net::SocketAddr) -> Result<()> {
        let n = self.listeners.len();
        let token = Token(n);

        let mut tcp_listener =
            TcpListener::bind(addr).map_err(|e| Error::StartListening(addr.to_string(), e))?;

        self.poll
            .registry()
            .register(&mut tcp_listener, token, Interest::READABLE)
            .map_err(|e| Error::StartListening(addr.to_string(), e))?;

        let addr = addr.to_string();
        logln!("Bound {addr}     // {token:?} -> ({:?})", tcp_listener);
        self.listeners.push((addr, tcp_listener));

        Ok(())
    }

    pub fn run(&mut self) -> Result<Proxy> {
        let mut events = Events::with_capacity(20);
        let mut i = 0u64;
        loop {
            i += 1;
            logln!("");
            logln!("▶ start event loop iteration {i}");
            self.poll.poll(&mut events, None).map_err(Error::Poll)?;
            logln!("  woke up with {n} events", n = events.iter().count());
            for ev in events.iter() {
                let token = ev.token();
                if token.0 < self.forward_token_start {
                    self.handle_listener_event(token.0)?;
                } else {
                    self.handle_forward_event(ev, (token.0 - self.forward_token_start) / 2);
                }
            }
        }
    }

    fn handle_listener_event(&mut self, n: usize) -> Result<()> {
        loop {
            let (local, listener) = &self.listeners[n];
            let local = local.clone();
            let (conn, peer) = match listener.accept() {
                Ok(x) => x,
                Err(e) if would_block(&e) => return Ok(()),
                Err(e) => {
                    return Err(Error::Accept(local.clone(), e));
                }
            };
            logln!("New connection on {local} from {peer}");
            if let Err(e) = self.start_forwarder(peer.to_string(), conn) {
                logln!("Could not proxy connection on {local} from {peer}: {e}");
            }
        }
    }

    fn start_forwarder(&mut self, peer: String, conn: TcpStream) -> Result<()> {
        let entry = self.forwarders.vacant_entry();
        let n = entry.key();
        let start = self.forward_token_start;
        let client_token = Token(start + 2 * n);
        let server_token = Token(start + 2 * n + 1);
        let fwd = Forwarder::new(
            self.poll.registry(),
            conn,
            peer,
            client_token,
            &self.forward_addr,
            server_token,
        );
        match fwd {
            Ok(forwarder) => {
                logln!(
                    "Starting forwarder {n} [{c}, {s}]",
                    c = client_token.0,
                    s = server_token.0
                );
                entry.insert(forwarder);
            }
            Err(e) => {
                logln!("{e}");
            }
        }
        Ok(())
    }

    fn handle_forward_event(&mut self, ev: &Event, n: usize) {
        let Proxy {
            poll, forwarders, ..
        } = self;
        let registry = poll.registry();
        let fwd = forwarders.get_mut(n).unwrap();

        match fwd.handle_event(registry, ev) {
            Ok(ControlFlow::Continue(_)) => return,
            Ok(ControlFlow::Break(_)) => {}
            Err(e) => {
                logln!("Connection {n}: {e}");
            }
        }

        // if we get here the forwarder must be dropped
        logln!("Dropping connection {n}");
        fwd.deregister(registry);
        self.forwarders.remove(n);
    }
}

struct ShowEvent<'a>(&'a Event);

impl fmt::Display for ShowEvent<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use fmt::Write;
        let ev = self.0;
        let mut s = String::with_capacity(12);
        if ev.is_readable() {
            s.push('R');
        }
        if ev.is_writable() {
            s.push('W');
        }
        if ev.is_error() {
            s.push('E');
        }
        if ev.is_read_closed() {
            s.push_str("rc");
        }
        if ev.is_write_closed() {
            s.push_str("wc");
        }
        if ev.is_priority() {
            s.push('P');
        }
        if ev.is_aio() {
            s.push('A');
        }
        if ev.is_lio() {
            s.push('L');
        }

        write!(f, "Event({n}, {s:?})", n = ev.token().0)
    }
}

fn would_block(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
}

fn err_is_interrupted(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Interrupted
}
