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

use super::{network::Addr, would_block, Error, Result};

pub struct Forwarder(Option<Forwarding>);

#[derive(Debug)]
enum Forwarding {
    Connecting(Connecting),
    Running(Running),
}

impl Forwarder {
    pub fn new(
        registry: &Registry,
        conn: TcpStream,
        peer: String,
        client_token: Token,
        forward_addr: &Addr,
        server_token: Token,
    ) -> Result<Self> {
        let connecting = Connecting::new(
            forward_addr,
            peer,
            client_token,
            conn,
            server_token,
            registry,
        )?;
        Ok(Forwarder(Some(Forwarding::Connecting(connecting))))
    }

    pub fn deregister(&mut self, registry: &Registry) {
        match &mut self.0 {
            Some(Forwarding::Connecting(c)) => c.deregister(registry),
            Some(Forwarding::Running(r)) => r.deregister(registry),
            None => {}
        }
    }

    pub fn handle_event(&mut self, registry: &Registry, ev: &Event) -> Result<ControlFlow<()>> {
        logln!("processing {ev:?}");

        let old = self
            .0
            .take()
            .expect("must have lost connection state earlier");
        let handled: ControlFlow<(), Forwarding> = match old {
            Forwarding::Connecting(c) => c.process(registry)?,
            Forwarding::Running(r) => r.process(registry)?,
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
    pub client: Registered<TcpStream>,
    pub server: Registered<TcpStream>,
}

impl Connecting {
    fn new(
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
        let addr = addrs[0];
        logln!("Trying to connect to {addr}");
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

    fn process(mut self, registry: &Registry) -> Result<ControlFlow<(), Forwarding>> {
        let Connecting { server, .. } = &mut self;

        // If there's a true error, return it
        let server_status = server.attempt(Interest::WRITABLE, |conn| conn.take_error());
        if let Err(e) | Ok(Some(e)) = server_status {
            return Err(Error::Connect(server.name.clone(), e));
        }

        // Check peer_name to see if we're really connected.
        let err = match server.attempt(Interest::WRITABLE, |conn| conn.peer_addr()) {
            Ok(_peer_name) => {
                logln!("Connected to {remote}", remote = server.name.clone());
                let running = Running::from(self);
                return running.process(registry);
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
    pub client: Registered<TcpStream>,
    pub server: Registered<TcpStream>,
    pub upstream: Copying,
    pub downstream: Copying,
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

    fn process(mut self, registry: &Registry) -> Result<ControlFlow<(), Forwarding>> {
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

            progress |= downstream.handle_one("downstream", server, client)?;
            progress |= upstream.handle_one("upstream", client, server)?;
            logln!(
                "client interest {c:?}, server interest {s:?}",
                c = client.needed,
                s = server.needed,
            );
        }

        client
            .update_registration(registry)
            .map_err(Error::Forward)?;
        server
            .update_registration(registry)
            .map_err(Error::Forward)?;

        let uf = upstream.finished();
        let df = downstream.finished();
        logln!("upstream finished = {uf}, downstream finished = {df}");
        if uf && df {
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
        direction: &str,
        rd: &mut Registered<TcpStream>,
        wr: &mut Registered<TcpStream>,
    ) -> Result<bool> {
        assert!(self.unsent_data <= self.free_space);
        assert!(self.free_space <= Self::BUFSIZE);
        assert!(self.unsent_data == self.free_space || self.can_write);

        let mut progress = false;

        logln!(
            "{direction}: can_read={r} can_write={w}    0 ≤ {unsent} ≤ {free} ≤ {size}",
            r = self.can_read,
            w = self.can_write,
            unsent = self.unsent_data,
            free = self.free_space,
            size = Self::BUFSIZE,
        );
        let to_write = &self.buffer[self.unsent_data..self.free_space];
        if !to_write.is_empty() {
            assert!(self.can_write);
            logln!("  trying to write");
            match wr.attempt(Interest::WRITABLE, |w| w.write(to_write)) {
                Ok(n @ 1..) => {
                    progress = true;
                    self.unsent_data += n;
                    logln!("  sent {n} bytes");
                }
                Ok(0) => {
                    // eof
                    progress = true;
                    let n = self.free_space - self.unsent_data;
                    logln!("  can no longer write, discarding {n} bytes");
                    self.unsent_data = self.free_space;
                    self.can_write = false;
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
                // no data in the buffer and no option to get more
                logln!("  shutting down writes");
                self.can_write = false;
                let _ = wr.source.shutdown(std::net::Shutdown::Write);
            }
            if self.can_read && !self.can_write {
                logln!("  shutting down reads");
                self.can_read = false;
                let _ = rd.source.shutdown(std::net::Shutdown::Read);
            }
        }

        if self.can_read && self.can_write && self.free_space < Self::BUFSIZE {
            logln!("  trying to read");
            let dest = &mut self.buffer[self.free_space..];
            match rd.attempt(Interest::READABLE, |r| r.read(dest)) {
                Ok(n @ 1..) => {
                    let data = &dest[..n];
                    let data = String::from_utf8_lossy(data);
                    logln!("  received {n} bytes: {data:?}");
                    progress = true;
                    self.free_space += n;
                }
                Ok(0) => {
                    // eof
                    logln!("  received eof");
                    progress = true;
                    self.can_read = false;
                }
                Err(e) if would_block(&e) => {
                    // don't touch progress
                }
                Err(e) => return Err(Error::Forward(e)),
            }
        }

        logln!("  progress is {progress:?}");
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
