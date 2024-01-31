use std::{
    io::{self, ErrorKind, Read, Write},
    net::{SocketAddr, ToSocketAddrs},
    ops::ControlFlow::{self, Break, Continue},
};

use mio::{
    event::{Event, Source},
    net::TcpStream,
    Interest, Registry, Token,
};

use super::{
    event::{ConnectionId, ConnectionSink, Direction},
    network::Addr,
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
        conn: TcpStream,
        peer: String,
        client_token: Token,
        forward_addr: &Addr,
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
        let old = self.0.take().unwrap();
        let handled: ControlFlow<(), Forwarding> = match old {
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
    client: Registered<TcpStream>,
    server: Registered<TcpStream>,
}

impl Connecting {
    fn new(
        event_sink: &mut ConnectionSink,
        server_addr: &Addr,
        client_addr: String,
        client_token: Token,
        client: TcpStream,
        server_token: Token,
        registry: &Registry,
    ) -> Result<Connecting> {
        let Addr::Tcp(server_addr) = server_addr else {
            return Err(Error::UnixDomain);
        };
        let addrs: Vec<SocketAddr> = server_addr
            .to_socket_addrs()
            .map_err(|e| Error::Connect(server_addr.to_string(), e))?
            .collect();
        if addrs.is_empty() {
            let msg = "name does not resolve to any addresses";
            let e = io::Error::new(ErrorKind::NotFound, msg);
            return Err(Error::Connect(server_addr.to_string(), e));
        }

        // TODO: Should really connect to all of them
        let addr = addrs[0];
        event_sink.emit_connecting(addr.to_string());
        let conn =
            TcpStream::connect(addr).map_err(|e| Error::Connect(server_addr.to_string(), e))?;

        let client = Registered::new(client_addr, client_token, client);
        let mut server = Registered::new(server_addr.to_string(), server_token, conn);
        server.need(Some(Interest::WRITABLE));
        server
            .update_registration(registry)
            .map_err(|e| Error::Connect(server.name.clone(), e))?;
        let connecting = Connecting { client, server };
        Ok(connecting)
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
        let Connecting { server, .. } = &mut self;

        // If there's a true error, return it
        let server_status = server.attempt(Interest::WRITABLE, |conn| conn.take_error());
        if let Err(e) | Ok(Some(e)) = server_status {
            return Err(Error::Connect(server.name.clone(), e));
        }

        // Check peer_name to see if we're really connected.
        let err = match server.attempt(Interest::WRITABLE, |conn| conn.peer_addr()) {
            Ok(peer_name) => {
                sink.emit_connected(peer_name.to_string());
                let running = Running::from(self);
                return running.process(sink, registry);
            }
            Err(e) => e,
        };

        // Some error kinds mean 'continue trying', others mean 'abort'.
        if let ErrorKind::WouldBlock | ErrorKind::NotConnected = err.kind() {
            let forwarding = Forwarding::Connecting(self);
            Ok(Continue(forwarding))
        } else {
            Err(Error::Connect(server.name.clone(), err))
        }
    }
}

#[derive(Debug)]
struct Running {
    client: Registered<TcpStream>,
    server: Registered<TcpStream>,
    upstream: Copying,
    downstream: Copying,
}

impl Running {
    fn from(connecting: Connecting) -> Running {
        let Connecting { client, server } = connecting;
        let upstream = Copying::new();
        let downstream = Copying::new();
        Running {
            client,
            server,
            upstream,
            downstream,
        }
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
            .map_err(Error::Forward)?;
        server
            .update_registration(registry)
            .map_err(Error::Forward)?;

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
}

impl Copying {
    const BUFSIZE: usize = 8192;

    fn new() -> Self {
        Copying {
            can_read: true,
            can_write: true,
            buffer: Box::new([0; Self::BUFSIZE]),
            unsent_data: 0,
            free_space: 0,
        }
    }

    fn handle_one(
        &mut self,
        direction: Direction,
        sink: &mut ConnectionSink,
        rd: &mut Registered<TcpStream>,
        wr: &mut Registered<TcpStream>,
    ) -> Result<bool> {
        assert!(self.unsent_data <= self.free_space);
        assert!(self.free_space <= Self::BUFSIZE);
        assert!(self.unsent_data == self.free_space || self.can_write);

        let mut progress = false;

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
                Err(e) => return Err(Error::Forward(e)),
            }
        }

        if self.unsent_data == self.free_space {
            self.unsent_data = 0;
            self.free_space = 0;
            if self.can_write && !self.can_read {
                // No data in the buffer and no option to get more
                // Do not emit ShutdownWrite because the user has already seen
                // the ShutdownRead
                // sink.emit_shutdown_write(direction, 0);
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
                    sink.emit_data(direction, data.to_vec());
                    progress = true;
                    self.free_space += n;
                }
                Ok(0) => {
                    // eof
                    sink.emit_data(direction, vec![]);
                    progress = true;
                    sink.emit_shutdown_read(direction);
                    self.can_read = false;
                    let _ = rd.source.shutdown(std::net::Shutdown::Read);
                }
                Err(e) if would_block(&e) => {
                    // don't touch progress
                }
                Err(e) => return Err(Error::Forward(e)),
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
