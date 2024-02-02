use std::{
    io::{self, ErrorKind, Read, Write},
    ops::ControlFlow::{self, Break, Continue},
    vec,
};

use mio::{
    event::{Event, Source},
    Interest, Registry, Token,
};

use super::{
    event::{ConnectionId, ConnectionSink, Direction},
    network::{Addr, MioStream, MonetAddr},
    would_block, Error, Result,
};

pub struct Forwarder(Option<Forwarding>, ConnectionId);

#[derive(Debug)]
enum Forwarding {
    Connecting(Connecting),
    Running(Running),
}

impl Forwarder {
    pub fn new(
        registry: &Registry,
        event_sink: &mut ConnectionSink,
        conn: MioStream,
        peer: Addr,
        client_token: Token,
        forward_addr: &MonetAddr,
        server_token: Token,
    ) -> Result<Self> {
        let connecting = Connecting::new(
            event_sink,
            forward_addr,
            peer,
            client_token,
            conn,
            server_token,
            registry,
        )?;
        let forwarding = Forwarding::Connecting(connecting);
        let forwarder = Forwarder(Some(forwarding), event_sink.id());
        Ok(forwarder)
    }

    pub fn id(&self) -> ConnectionId {
        self.1
    }

    pub fn deregister(&mut self, registry: &Registry) {
        match &mut self.0 {
            Some(Forwarding::Connecting(c)) => c.deregister(registry),
            Some(Forwarding::Running(r)) => r.deregister(registry),
            None => {}
        }
    }

    pub fn handle_event(
        &mut self,
        sink: &mut ConnectionSink,
        registry: &Registry,
        _ev: &Event,
    ) -> Result<ControlFlow<()>> {
        let old_state = self.0.take().unwrap();
        let handled: ControlFlow<(), Forwarding> = match old_state {
            Forwarding::Connecting(c) => c.process(sink, registry)?,
            Forwarding::Running(r) => r.process(sink, registry)?,
        };
        match handled {
            Continue(forwarding) => {
                self.0 = Some(forwarding);
                Ok(Continue(()))
            }
            Break(()) => Ok(Break(())),
        }
    }
}

#[derive(Debug)]
struct Connecting {
    client: Registered<MioStream>,
    server: Registered<MioStream>,
    addrs: vec::IntoIter<Addr>,
}

impl Connecting {
    fn new(
        event_sink: &mut ConnectionSink,
        server_addr: &MonetAddr,
        client_addr: Addr,
        client_token: Token,
        client: MioStream,
        server_token: Token,
        registry: &Registry,
    ) -> Result<Connecting> {
        let addrs = match server_addr.resolve() {
            Ok(addrs) => addrs,
            Err(e) => {
                event_sink.emit_connect_failed(server_addr.to_string(), true, e);
                return Err(Error::Connect);
            }
        };

        if addrs.is_empty() {
            let msg = "name does not resolve to any addresses";
            let e = io::Error::new(ErrorKind::NotFound, msg);
            event_sink.emit_connect_failed(server_addr.to_string(), true, e);
            return Err(Error::Connect);
        }

        let client = Registered::new(client_addr.to_string(), client_token, client);

        let mut addrs = addrs.into_iter();
        let Some(server) = Self::connect_addrs(event_sink, server_token, registry, &mut addrs)
        else {
            return Err(Error::Connect);
        };

        let connecting = Connecting {
            client,
            server,
            addrs,
        };
        Ok(connecting)
    }

    /// Try to connect to each of the addrs in turn, returning when one succeeds.
    ///
    /// If all fail, return the last error.
    /// If there were no addrs left, return Ok(Some).
    fn connect_addrs(
        event_sink: &mut ConnectionSink,
        token: Token,
        registry: &Registry,
        addrs: impl Iterator<Item = Addr>,
    ) -> Option<Registered<MioStream>> {
        for addr in addrs {
            event_sink.emit_connecting(addr.clone());
            let err = match addr.connect() {
                Ok(stream) => {
                    let mut server = Registered::new(addr.to_string(), token, stream);
                    server.need(Some(Interest::WRITABLE));
                    match server.update_registration(registry) {
                        Ok(()) => return Some(server),
                        Err(e) => e,
                    }
                }
                Err(e) => e,
            };
            event_sink.emit_connect_failed(addr.to_string(), true, err);
        }
        None
    }

    fn deregister(&mut self, registry: &Registry) {
        let _ = self.client.deregister(registry);
        let _ = self.server.deregister(registry);
    }

    fn process(
        self,
        sink: &mut ConnectionSink,
        registry: &Registry,
    ) -> Result<ControlFlow<(), Forwarding>> {
        let Connecting {
            client,
            mut server,
            mut addrs,
        } = self;

        let established = server.attempt(Interest::WRITABLE, |conn| conn.established());

        // If it succeeded or if we're still waiting, handle that here.
        // Otherwise, we'll have to report the error and try another address
        let error = match established {
            Ok(Some(peer)) => {
                sink.emit_connected(peer);
                let running = Running::from(client, server)?;
                // kickstart it by running its process method too
                return running.process(sink, registry);
            }
            Ok(None) => {
                let connecting = Connecting {
                    client,
                    server,
                    addrs,
                };
                let forwarding = Forwarding::Connecting(connecting);
                return Ok(Continue(forwarding));
            }
            Err(e) => e,
        };

        sink.emit_connect_failed(server.name.clone(), false, error);

        let token = server.token;
        drop(server);

        if let Some(server) = Self::connect_addrs(sink, token, registry, &mut addrs) {
            let connecting = Connecting {
                client,
                server,
                addrs,
            };
            let forwarding = Forwarding::Connecting(connecting);
            Ok(Continue(forwarding))
        } else {
            Err(Error::Connect)
        }
    }
}

#[derive(Debug)]
struct Running {
    client: Registered<MioStream>,
    server: Registered<MioStream>,
    upstream: Copying,
    downstream: Copying,
}

impl Running {
    fn from(client: Registered<MioStream>, server: Registered<MioStream>) -> Result<Running> {
        let client_is_unix = client.source.is_unix();
        let server_is_unix = server.source.is_unix();
        let upstream = Copying::new(client_is_unix, server_is_unix);
        let downstream = Copying::new(false, false);

        for (side, sock) in [("client", &client), ("server", &server)] {
            sock.source.set_nodelay(true).map_err(|e| Error::Forward {
                doing: "setting nodelay",
                side,
                err: e,
            })?;
        }

        let running = Running {
            client,
            server,
            upstream,
            downstream,
        };
        Ok(running)
    }

    fn deregister(&mut self, registry: &Registry) {
        let _ = self.client.deregister(registry);
        let _ = self.server.deregister(registry);
    }

    fn process(
        mut self,
        sink: &mut ConnectionSink,
        registry: &Registry,
    ) -> Result<ControlFlow<(), Forwarding>> {
        let Running {
            client,
            server,
            upstream,
            downstream,
        } = &mut self;

        let mut progress = true;
        while progress {
            progress = false;
            client.clear();
            server.clear();

            progress |= downstream.handle_one(Direction::Downstream, sink, server, client)?;
            progress |= upstream.handle_one(Direction::Upstream, sink, client, server)?;
        }

        client
            .update_registration(registry)
            .map_err(|err| Error::Forward {
                doing: "registering",
                side: "client",
                err,
            })?;
        server
            .update_registration(registry)
            .map_err(|err| Error::Forward {
                doing: "registering",
                side: "server",
                err,
            })?;

        if upstream.finished() && downstream.finished() {
            Ok(Break(()))
        } else {
            Ok(Continue(Forwarding::Running(self)))
        }
    }
}

#[derive(Debug)]
pub struct Copying {
    can_read: bool,
    can_write: bool,
    buffer: Box<[u8; Self::BUFSIZE]>,
    unsent_data: usize,
    free_space: usize,
    fix_unix_read: bool,
}

impl Copying {
    const BUFSIZE: usize = 8192;

    fn new(fix_unix_read: bool, fix_unix_write: bool) -> Self {
        let mut free_space = 0;
        let mut buffer = Box::new([0; Self::BUFSIZE]);

        if fix_unix_write {
            buffer[0] = b'0';
            free_space = 1;
        }

        Copying {
            can_read: true,
            can_write: true,
            buffer,
            unsent_data: 0,
            free_space,
            fix_unix_read,
        }
    }

    fn handle_one(
        &mut self,
        direction: Direction,
        sink: &mut ConnectionSink,
        rd: &mut Registered<MioStream>,
        wr: &mut Registered<MioStream>,
    ) -> Result<bool> {
        assert!(self.unsent_data <= self.free_space);
        assert!(self.free_space <= Self::BUFSIZE);
        assert!(self.unsent_data == self.free_space || self.can_write);

        let mut progress = false;

        if self.fix_unix_read && self.free_space > 0 {
            assert_eq!(self.unsent_data, 0);
            if self.buffer[0] == b'0' {
                // skip it
                self.unsent_data = 1;
                self.fix_unix_read = false;
            } else {
                return Err(Error::Other(
                    "client did not start with a '0' (0x30) byte".to_string(),
                ));
            }
        }

        let to_write = &self.buffer[self.unsent_data..self.free_space];
        if !to_write.is_empty() {
            assert!(self.can_write);
            match wr.attempt(Interest::WRITABLE, |w| w.write(to_write)) {
                Ok(n @ 1..) => {
                    progress = true;
                    self.unsent_data += n;
                }
                Ok(0) => {
                    // eof
                    progress = true;
                    let n = self.free_space - self.unsent_data;
                    sink.emit_shutdown_write(direction, n);
                    self.unsent_data = self.free_space;
                    self.can_write = false;
                    let _ = wr.source.shutdown(std::net::Shutdown::Write);
                }
                Err(e) if would_block(&e) => {
                    // don't touch progress
                }
                Err(err) => {
                    return Err(Error::Forward {
                        doing: "writing",
                        side: direction.receiver(),
                        err,
                    })
                }
            }
        }

        if self.unsent_data == self.free_space {
            self.unsent_data = 0;
            self.free_space = 0;
            if self.can_write && !self.can_read {
                // No data in the buffer and no option to get more
                self.can_write = false;
                let _ = wr.source.shutdown(std::net::Shutdown::Write);
            }
            if self.can_read && !self.can_write {
                sink.emit_shutdown_read(direction);
                self.can_read = false;
                let _ = rd.source.shutdown(std::net::Shutdown::Read);
            }
        }

        if self.can_read && self.can_write && self.free_space < Self::BUFSIZE {
            let dest = &mut self.buffer[self.free_space..];
            match rd.attempt(Interest::READABLE, |r| r.read(dest)) {
                Ok(n @ 1..) => {
                    let data = &dest[..n];
                    sink.emit_data(direction, data);
                    progress = true;
                    self.free_space += n;
                }
                Ok(0) => {
                    // eof
                    progress = true;
                    sink.emit_shutdown_read(direction);
                    self.can_read = false;
                    let _ = rd.source.shutdown(std::net::Shutdown::Read);
                }
                Err(e) if would_block(&e) => {
                    // don't touch progress
                }
                Err(err) => {
                    return Err(Error::Forward {
                        doing: "reading",
                        side: direction.sender(),
                        err,
                    })
                }
            }
        }

        Ok(progress)
    }

    fn finished(&self) -> bool {
        !self.can_read && !self.can_write
    }
}

#[derive(Debug)]
pub struct Registered<S: Source> {
    name: String,

    source: S,

    token: Token,

    /// Interests we would like to be ready. Will be added
    /// to our Poll Registry if not already in ready.
    needed: Option<Interest>,

    /// Interests we have registered with the Poll Registry.
    registered: Option<Interest>,
}

impl<S: Source> Registered<S> {
    fn new(name: String, token: Token, source: S) -> Self {
        Registered {
            name,
            source,
            token,
            needed: None,
            registered: None,
        }
    }

    fn clear(&mut self) {
        self.needed = None;
    }

    fn need(&mut self, interests: Option<Interest>) {
        self.needed = combine_interests(self.needed, interests);
    }

    fn attempt<T>(
        &mut self,
        interests: Interest,
        f: impl FnOnce(&mut S) -> io::Result<T>,
    ) -> io::Result<T> {
        let result = f(&mut self.source);
        if let Err(e) = &result {
            if would_block(e) {
                self.need(Some(interests))
            }
        }
        result
    }

    fn update_registration(&mut self, registry: &Registry) -> io::Result<()> {
        match (self.registered, self.needed) {
            (None, None) => {}
            (Some(_), None) => registry.deregister(&mut self.source)?,
            (None, Some(interests)) => {
                registry.register(&mut self.source, self.token, interests)?
            }
            (Some(old), Some(new)) => {
                if old != new {
                    registry.reregister(&mut self.source, self.token, new)?;
                }
            }
        }
        self.registered = self.needed;
        Ok(())
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        self.clear();
        self.update_registration(registry)
    }
}

fn combine_interests(left: Option<Interest>, right: Option<Interest>) -> Option<Interest> {
    match (left, right) {
        (None, x) => x,
        (y, None) => y,
        (Some(x), Some(y)) => Some(x | y),
    }
}

#[test]
fn test_interest_or() {
    let x = Some(Interest::READABLE);
    let y = Some(Interest::PRIORITY);
    let z = combine_interests(x, y);

    assert_eq!(z, Some(Interest::READABLE | Interest::PRIORITY));
}
