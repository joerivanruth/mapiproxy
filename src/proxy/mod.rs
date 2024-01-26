mod forward;
pub mod network;

use std::{
    io,
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

type Id = usize;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Direction {
    Upstream,
    Downstream,
}

#[derive(Debug)]
pub enum MapiEvent {
    BoundPort(String),
    Incoming {
        id: Id,
        local: String,
        peer: String,
    },
    Connecting {
        id: Id,
        remote: String,
    },
    Connected {
        id: Id,
        peer: String,
    },
    End {
        id: Id,
    },
    Aborted {
        id: Id,
        error: Error,
    },
    Data {
        id: Id,
        direction: Direction,
        data: Vec<u8>,
    },
    ShutdownRead {
        id: Id,
        direction: Direction,
    },
    ShutdownWrite {
        id: Id,
        direction: Direction,
        discard: usize,
    },
}

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

pub struct EventSink(Box<dyn FnMut(MapiEvent)>);

impl EventSink {
    pub fn new(f: impl FnMut(MapiEvent) + 'static) -> Self {
        EventSink(Box::new(f))
    }

    pub fn sub(&mut self, id: Id) -> ConnectionSink<'_> {
        ConnectionSink::new(&mut *self, id)
    }

    fn emit_event(&mut self, event: MapiEvent) {
        (self.0)(event)
    }

    pub fn emit_bound(&mut self, port: String) {
        self.emit_event(MapiEvent::BoundPort(port))
    }
}

pub struct ConnectionSink<'a>(&'a mut EventSink, Id);

impl<'a> ConnectionSink<'a> {
    pub fn new(event_sink: &'a mut EventSink, id: Id) -> Self {
        ConnectionSink(event_sink, id)
    }

    pub fn id(&self) -> Id {
        self.1
    }

    pub fn emit_incoming(&mut self, local: String, peer: String) {
        self.0.emit_event(MapiEvent::Incoming {
            id: self.id(),
            local,
            peer,
        });
    }

    pub fn emit_connecting(&mut self, remote: String) {
        self.0.emit_event(MapiEvent::Connecting {
            id: self.id(),
            remote,
        });
    }

    pub fn emit_connected(&mut self, remote: String) {
        self.0.emit_event(MapiEvent::Connected {
            id: self.id(),
            peer: remote,
        });
    }

    pub fn emit_end(&mut self) {
        self.0.emit_event(MapiEvent::End { id: self.id() });
    }

    pub fn emit_aborted(&mut self, error: Error) {
        self.0.emit_event(MapiEvent::Aborted {
            id: self.id(),
            error,
        });
    }

    pub fn emit_data(&mut self, direction: Direction, data: Vec<u8>) {
        self.0.emit_event(MapiEvent::Data {
            id: self.id(),
            direction,
            data,
        })
    }

    pub fn emit_shutdown_read(&mut self, direction: Direction) {
        self.0.emit_event(MapiEvent::ShutdownRead {
            id: self.id(),
            direction,
        });
    }

    pub fn emit_shutdown_write(&mut self, direction: Direction, discard: usize) {
        self.0.emit_event(MapiEvent::ShutdownWrite {
            id: self.id(),
            direction,
            discard,
        });
    }
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
            self.poll.poll(&mut events, None).map_err(Error::Poll)?;
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

            let id = self.ids.next().unwrap();
            self.event_sink
                .sub(id)
                .emit_incoming(local.clone(), peer.clone());
            self.start_forwarder(id, peer.to_string(), conn);
        }
    }

    fn start_forwarder(&mut self, id: Id, peer: String, conn: TcpStream) {
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
        let fwd = self.forwarders.get_mut(n).unwrap();
        let id: Id = fwd.id();
        let mut sink = self.event_sink.sub(id);

        match fwd.handle_event(&mut sink, registry, ev) {
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
        fwd.deregister(registry);
        self.forwarders.remove(n);
    }
}

fn would_block(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
}
