pub mod event;
mod forward;
pub mod network;

use std::{
    io::{self, ErrorKind},
    net::{self, ToSocketAddrs},
    ops::{ControlFlow, RangeFrom},
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

use self::event::{ConnectionId, EventSink};

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

    #[error("forwarding failed when {doing} {side}: {err}")]
    Forward {
        doing: &'static str,
        side: &'static str,
        err: io::Error,
    },

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
    ids: RangeFrom<usize>,
    event_sink: EventSink,
}

impl Proxy {
    pub fn new(listen_addr: Addr, forward_addr: Addr, event_sink: EventSink) -> Result<Proxy> {
        let poll = Poll::new().map_err(Error::CreatePoll)?;
        let mut proxy = Proxy {
            listen_addr,
            forward_addr,
            poll,
            token_base: usize::MAX,
            listeners: Default::default(),
            forwarders: Default::default(),
            ids: 10..,
            event_sink,
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
        self.event_sink.emit_bound(addr.clone());
        self.listeners.push((addr, tcp_listener));

        Ok(())
    }

    pub fn run(&mut self) -> Result<Proxy> {
        let mut events = Events::with_capacity(20);
        let mut _i = 0u64;
        loop {
            _i += 1;
            // self.poll.poll(&mut events, None).map_err(Error::Poll)?;
            match self.poll.poll(&mut events, None) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::Poll(e)),
            }
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
            let peer = peer.to_string();

            let id = ConnectionId::new(self.ids.next().unwrap());
            self.event_sink
                .sub(id)
                .emit_incoming(local.clone(), peer.clone());
            self.start_forwarder(id, peer.to_string(), conn);
        }
    }

    fn start_forwarder(&mut self, id: ConnectionId, peer: String, conn: TcpStream) {
        let mut sink = self.event_sink.sub(id);
        let entry = self.forwarders.vacant_entry();
        let n = entry.key();
        let client_token = self.token_base + 2 * n;
        let server_token = self.token_base + 2 * n + 1;
        let new = Forwarder::new(
            self.poll.registry(),
            &mut sink,
            conn,
            peer,
            Token(client_token),
            &self.forward_addr,
            Token(server_token),
        );
        match new {
            Ok(forwarder) => {
                entry.insert(forwarder);
            }
            Err(e) => {
                sink.emit_aborted(e);
            }
        }
    }

    fn handle_forward_event(&mut self, ev: &Event, n: usize) {
        let registry = self.poll.registry();
        let Some(forwarder) = self.forwarders.get_mut(n) else {
            return;
        };
        let id = forwarder.id();
        let mut sink = self.event_sink.sub(id);

        match forwarder.handle_event(&mut sink, registry, ev) {
            Ok(ControlFlow::Continue(_)) => {
                // return instead of removing it
                return;
            }
            Err(e) => {
                sink.emit_aborted(e);
                // fall through to removal
            }
            Ok(ControlFlow::Break(_)) => {
                sink.emit_end();
                // fall through to removal
            }
        }

        // Removal
        forwarder.deregister(registry);
        self.forwarders.remove(n);
    }
}

fn would_block(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
}
