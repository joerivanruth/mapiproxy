mod forward;
pub mod network;

use std::{
    io,
    net::{self, ToSocketAddrs},
    ops::ControlFlow,
};

use forward::Forwarder;
use network::Addr;

use mio::{
    event::Event,
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

    #[error("Unix domain sockets are not supported yet")]
    UnixDomain,

    #[allow(dead_code)]
    #[error("{0}")]
    Other(String),
}

type Result<T> = std::result::Result<T, Error>;

pub struct Proxy {
    listen_addr: Addr,
    forward_addr: Addr,
    poll: Poll,
    token_base: usize,
    listeners: Vec<(String, TcpListener)>,
    forwarders: Slab<Forwarder>,
}

impl Proxy {
    pub fn new(listen_addr: Addr, forward_addr: Addr) -> Result<Proxy> {
        let poll = Poll::new().map_err(Error::CreatePoll)?;
        let mut proxy = Proxy {
            listen_addr,
            forward_addr,
            poll,
            token_base: usize::MAX,
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
        self.token_base = n;
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
                if token.0 < self.token_base {
                    self.handle_listener_event(token.0)?;
                } else {
                    self.handle_forward_event(ev, (token.0 - self.token_base) / 2);
                }
            }
        }
    }

    fn handle_listener_event(&mut self, n: usize) -> Result<()> {
        loop {
            let (local, listener) = &self.listeners[n];
            let (conn, peer) = match listener.accept() {
                Ok(x) => x,
                Err(e) if would_block(&e) => return Ok(()),
                Err(e) => {
                    return Err(Error::Accept(local.clone(), e));
                }
            };
            logln!("New connection on {local} from {peer}");
            if let Err(e) = self.start_forwarder(peer.to_string(), conn) {
                let local = &self.listeners[n].0;
                logln!("Could not proxy connection on {local} from {peer}: {e}");
            }
        }
    }

    fn start_forwarder(&mut self, peer: String, conn: TcpStream) -> Result<()> {
        let entry = self.forwarders.vacant_entry();
        let n = entry.key();
        let client_token = self.token_base + 2 * n;
        let server_token = self.token_base + 2 * n + 1;
        let forwarder = Forwarder::new(
            self.poll.registry(),
            conn,
            peer,
            Token(client_token),
            &self.forward_addr,
            Token(server_token),
        )?;
        logln!("Starting forwarder {n} [{client_token}, {server_token}]");
        entry.insert(forwarder);
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
            Err(e) => {
                logln!("Connection {n}: {e}");
            }
            Ok(ControlFlow::Break(_)) => {
                // do not report but fall through to removing it
            }
        }

        logln!("Dropping connection {n}");
        fwd.deregister(registry);
        self.forwarders.remove(n);
    }
}

fn would_block(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
}
