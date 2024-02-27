pub mod event;
mod forward;
pub mod network;

use std::{
    io::{self, ErrorKind},
    ops::{ControlFlow, RangeFrom},
    sync::Arc,
};

use forward::Forwarder;
use network::Addr;

use mio::{event::Event, Events, Interest, Poll, Token};
use slab::Slab;
use thiserror::Error as ThisError;

use self::{
    event::{ConnectionId, EventSink, MapiEvent},
    network::{MioListener, MioStream, MonetAddr},
};

/// Errors that can occur in the [Proxy].
///
/// There is currently no clear distinction between errors that are simply
/// reported as part of the event stream and errors that terminate the proxy.
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("Could not create mio poller: {0}")]
    CreatePoll(io::Error),

    #[error("Could not listen on {0}: {1}")]
    StartListening(String, io::Error),

    #[error("Could not poll: {0}")]
    Poll(io::Error),

    #[error("accept() failed on {0}: {1}")]
    Accept(Addr, io::Error),

    #[error("None of the servers responded")]
    Connect,

    #[error("forwarding failed when {doing} {side}: {err}")]
    Forward {
        doing: &'static str,
        side: &'static str,
        err: io::Error,
    },

    #[error("{0}")]
    Other(String),
}

type Result<T> = std::result::Result<T, Error>;

/// The Proxy listens on a number of sockets, forwards the connections
/// to another server and reports on the traffic as a series of
/// [MapiEvent]s.
pub struct Proxy {
    /// Configured address to listen on. May map to multiple concrete addresses,
    /// the proxy will listen on all of them
    listen_addr: MonetAddr,
    /// Configured address to forward to. May map to multiple concrete addresses,
    /// the proxy will try each in turn.
    forward_addr: MonetAddr,
    /// The mio Poll object used to multiplex all IO on a single thread.
    poll: Poll,
    /// The waker can be used to trigger the proxy externally, we use it
    /// to stop the proxy on Control-C.
    waker: Arc<mio::Waker>,
    /// mio Tokens below this number are belong to listeners, the rest belong
    /// to forwarded connections.
    token_base: usize,
    /// Holds ownership of the listeners. `Token(t)` maps to `listeners[t]`.
    listeners: Vec<(Addr, MioListener)>,
    /// Holds ownership of the forwarders. `Token(t+self.token_base)` maps to
    /// `forwarders[t/2]`.
    forwarders: Slab<Forwarder>,
    /// Iterator that yields fresh connection id's.
    ids: RangeFrom<usize>,
    /// This is where events are reported.
    event_sink: EventSink,
}

impl Proxy {
    const TRIGGER_SHUTDOWN_TOKEN: Token = Token(usize::MAX);

    /// Create a new Proxy which listens on the TCP/IPv4, TCP/IPv6 and Unix Domain
    /// sockets denoted by `listen_addr`. Returns an error if the listen sockets
    /// could not be bound. Use [Proxy::run] to start forwarding.
    pub fn new(
        listen_addr: MonetAddr,
        forward_addr: MonetAddr,
        event_handler: impl FnMut(MapiEvent) + 'static + Send,
    ) -> Result<Proxy> {
        let poll = Poll::new().map_err(Error::CreatePoll)?;
        let waker = mio::Waker::new(poll.registry(), Self::TRIGGER_SHUTDOWN_TOKEN)
            .map_err(Error::CreatePoll)?;
        let waker = Arc::new(waker);
        let mut proxy = Proxy {
            listen_addr,
            forward_addr,
            poll,
            waker,
            token_base: usize::MAX,
            listeners: Default::default(),
            forwarders: Default::default(),
            ids: 10..,
            event_sink: EventSink::new(event_handler),
        };

        proxy.add_listeners()?;
        Ok(proxy)
    }

    fn add_listeners(&mut self) -> Result<()> {
        let addrs = self
            .listen_addr
            .resolve()
            .map_err(|e| Error::StartListening(self.listen_addr.to_string(), e))?;

        if addrs.is_empty() {
            let err = io::Error::new(ErrorKind::NotFound, "listen address not found");
            return Err(Error::StartListening(self.listen_addr.to_string(), err));
        }
        for addr in addrs {
            self.add_tcp_listener(addr)?;
        }

        let n = self.listeners.len();
        self.token_base = n;
        Ok(())
    }

    fn add_tcp_listener(&mut self, addr: Addr) -> Result<()> {
        let n = self.listeners.len();
        let token = Token(n);

        let mut listener = addr
            .listen()
            .map_err(|e| Error::StartListening(addr.to_string(), e))?;

        self.poll
            .registry()
            .register(&mut listener, token, Interest::READABLE)
            .map_err(|e| Error::StartListening(addr.to_string(), e))?;

        self.event_sink.emit_bound(addr.clone());
        self.listeners.push((addr, listener));

        Ok(())
    }

    /// Run the Proxy's main loop. This will block until the result of a call to [Proxy::get_shutdown_trigger]
    /// is used to trigger a shutdown.
    pub fn run(&mut self) -> Result<()> {
        let mut events = Events::with_capacity(20);
        loop {
            match self.poll.poll(&mut events, None) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::Poll(e)),
            }
            for ev in events.iter() {
                let token = ev.token();
                if token == Self::TRIGGER_SHUTDOWN_TOKEN {
                    return Ok(());
                } else if token.0 < self.token_base {
                    self.handle_listener_event(token.0)?;
                } else {
                    self.handle_forward_event(ev, (token.0 - self.token_base) / 2);
                }
            }
        }
    }

    /// Obtain a shutdown trigger that when called, will end the main loop of [Proxy::run].
    pub fn get_shutdown_trigger(&mut self) -> Box<dyn Fn() + Send + Sync + 'static> {
        let waker = Arc::clone(&self.waker);
        Box::new(move || {
            if let Err(e) = waker.wake() {
                eprintln!("Failed to shut down the proxy: {e}");
            }
        })
    }

    fn handle_listener_event(&mut self, n: usize) -> Result<()> {
        // When mio notifies us of readiness may only re-enter mio when we
        // have observed an EWOULDBLOCK. Hence the loop.
        loop {
            let (local, listener) = &self.listeners[n];
            let (conn, peer) = match listener.accept() {
                Ok(x) => x,
                Err(e) if would_block(&e) => return Ok(()),
                Err(e) => {
                    return Err(Error::Accept(local.clone(), e));
                }
            };

            let id = ConnectionId::new(self.ids.next().unwrap());
            self.event_sink
                .connection_sink(id)
                .emit_incoming(local.clone(), peer.clone());
            self.start_forwarder(id, peer, conn);
        }
    }

    fn start_forwarder(&mut self, id: ConnectionId, peer: Addr, conn: MioStream) {
        let mut sink = self.event_sink.connection_sink(id);
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
        let mut sink = self.event_sink.connection_sink(id);

        // As with [handle_listener_event], when mio notifies us of readiness
        // may only re-enter mio when we have observed an EWOULDBLOCK. However,
        // we don't have a loop right here because `Forwarder::handle_event`
        // does the looping. It returns a `ControlFlow` to indicate whether
        // this connection needs to stay around or whether it can be removed.
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
