use std::{
    io::{self, ErrorKind, Read, Write},
    mem,
    net::{SocketAddr, ToSocketAddrs},
    ops::ControlFlow::{self, Break, Continue},
    vec,
};

use mio::{
    event::{Event, Source},
    net::TcpStream,
    Interest, Registry, Token,
};

use crate::{network::Addr, proxy::would_block};

use super::{Error, Result};

pub enum Forwarder {
    Connecting {
        client: Registered<TcpStream>,
        server: Registered<TcpStream>,
    },
    Running {
        client: Registered<TcpStream>,
        server: Registered<TcpStream>,
        upstream: Copying,
        downstream: Copying,
    },
    Dead,
}

impl Forwarder {
    pub fn new(
        registry: &Registry,
        client: TcpStream,
        client_addr: String,
        client_token: Token,
        server_addr: &Addr,
        server_token: Token,
    ) -> Result<Self> {
        let Addr::Tcp(server_addr) = server_addr else {
            return Err(Error::UnixDomain);
        };
        // let server_addr = server_address.as_str();

        // Wonder why mio doesn't offer a non-blocking name resolution API
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

        // registration is the last thing we do so we don't have to undo it if any of the above failed.
        server.need(Some(Interest::WRITABLE));
        server
            .update_registration(registry)
            .map_err(|e| Error::Connect(server.name.clone(), e))?;

        Ok(Forwarder::Connecting { client, server })
    }

    pub fn deregister(&mut self, registry: &Registry) {
        match self {
            Forwarder::Connecting { client, server } => {
                let _ = client.deregister(registry);
                let _ = server.deregister(registry);
            }
            Forwarder::Running { client, server, .. } => {
                let _ = client.deregister(registry);
                let _ = server.deregister(registry);
            }
            Forwarder::Dead => {}
        }
    }

    pub fn handle_event(&mut self, registry: &Registry, event: &Event) -> Result<ControlFlow<()>> {
        match self {
            Forwarder::Connecting { .. } => self.handle_connecting(registry, event),
            Forwarder::Running { .. } => self.handle_running(registry, event),
            Forwarder::Dead => Ok(Break(())),
        }
    }

    fn handle_connecting(
        &mut self,
        registry: &Registry,
        _event: &Event,
    ) -> std::prelude::v1::Result<ControlFlow<()>, Error> {
        let Forwarder::Connecting { server, .. } = self else {
            panic!("only call this on connecting forwarders")
        };

        // If there's a true error, return it
        if let Err(e) | Ok(Some(e)) = server.attempt(Interest::WRITABLE, |conn| conn.take_error()) {
            return Err(Error::Connect(server.name.clone(), e));
        }

        // Check peer_name to see if we're really connected.
        match server.attempt(Interest::WRITABLE, |conn| conn.peer_addr()) {
            Ok(_) => self.switch_to_running(registry),
            Err(e) => match e.kind() {
                ErrorKind::WouldBlock | ErrorKind::NotConnected => Ok(Continue(())),
                _ => Err(Error::Connect(server.name.clone(), e)),
            },
        }
    }

    fn switch_to_running(&mut self, registry: &Registry) -> Result<ControlFlow<()>> {
        let upstream = Copying::new();
        let downstream = Copying::new();

        // complicated dance to be able to replace self
        let mut tmp = Forwarder::Dead;
        mem::swap(&mut tmp, self);
        let Forwarder::Connecting { client, server } = tmp else {
            panic!("only call this on connecting forwarders")
        };

        let remote = server.name.clone();

        *self = Forwarder::Running {
            client,
            server,
            upstream,
            downstream,
        };

        logln!("Connected to {remote}");
        // Process_running will set the registrations right
        self.process_running(registry)
    }

    fn handle_running(&mut self, registry: &Registry, event: &Event) -> Result<ControlFlow<()>> {
        logln!("processing {event:?}");
        self.process_running(registry)
    }

    fn process_running(&mut self, registry: &Registry) -> Result<ControlFlow<()>> {
        let Forwarder::Running {
            client,
            server,
            upstream,
            downstream,
        } = self
        else {
            panic!("only call this on connecting forwarders")
        };

        let mut progress = true;
        while progress {
            progress = false;
            client.clear();
            server.clear();

            progress |= downstream.handle1("downstream", server, client)?;
            progress |= upstream.handle1("upstream", client, server)?;
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

        let upstream_finished = upstream.finished();
        let downstream_finished = downstream.finished();
        logln!("upstream finished = {upstream_finished}, downstream finished = {downstream_finished}");
        if upstream_finished && downstream_finished {
            Ok(Break(()))
        } else {
            Ok(Continue(()))
        }
    }
}

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

    fn handle1(
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
                    logln!("  received {n} bytes");
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
        self.needed = self.needed.or(interests);
    }

    fn dont_need(&mut self, interests: Option<Interest>) {
        // if interests is empty we don't need to do anything
        let Some(interests) = interests else {
            return;
        };

        if let Some(needed) = &mut self.needed {
            self.needed = needed.remove(interests);
        }
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
