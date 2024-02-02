use std::{fmt, io};

use smallvec::SmallVec;

use super::{network::Addr, Error};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct ConnectionId(usize);

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{n}", n = self.0)
    }
}

impl ConnectionId {
    pub fn new(n: usize) -> Self {
        ConnectionId(n)
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Direction {
    Upstream,
    Downstream,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Direction::Upstream => "UPSTREAM",
            Direction::Downstream => "DOWNSTREAM",
        };
        s.fmt(f)
    }
}

impl Direction {
    pub const CLIENT: &'static str = "client";

    pub const SERVER: &'static str = "server";

    pub fn sender(&self) -> &'static str {
        match self {
            Direction::Upstream => Self::CLIENT,
            Direction::Downstream => Self::SERVER,
        }
    }

    #[allow(dead_code)]
    pub fn receiver(&self) -> &'static str {
        match self {
            Direction::Upstream => Self::SERVER,
            Direction::Downstream => Self::CLIENT,
        }
    }
}

#[derive(Debug)]
pub enum MapiEvent {
    BoundPort(Addr),
    Incoming {
        id: ConnectionId,
        local: Addr,
        peer: Addr,
    },
    Connecting {
        id: ConnectionId,
        remote: Addr,
    },
    Connected {
        id: ConnectionId,
        peer: Addr,
    },
    End {
        id: ConnectionId,
    },
    Aborted {
        id: ConnectionId,
        error: Error,
    },
    Data {
        id: ConnectionId,
        direction: Direction,
        data: SmallVec<[u8; 8]>,
    },
    ShutdownRead {
        id: ConnectionId,
        direction: Direction,
    },
    ShutdownWrite {
        id: ConnectionId,
        direction: Direction,
        discard: usize,
    },
    ConnectFailed {
        id: ConnectionId,
        remote: String,
        error: io::Error,
        immediately: bool,
    },
}

pub struct EventSink(Box<dyn FnMut(MapiEvent) + 'static + Send>);

impl EventSink {
    pub fn new(f: impl FnMut(MapiEvent) + 'static + Send) -> Self {
        EventSink(Box::new(f))
    }

    pub fn sub(&mut self, id: ConnectionId) -> ConnectionSink<'_> {
        ConnectionSink::new(&mut *self, id)
    }

    fn emit_event(&mut self, event: MapiEvent) {
        (self.0)(event)
    }

    pub fn emit_bound(&mut self, port: Addr) {
        self.emit_event(MapiEvent::BoundPort(port))
    }
}

pub struct ConnectionSink<'a>(&'a mut EventSink, ConnectionId);

impl<'a> ConnectionSink<'a> {
    pub fn new(event_sink: &'a mut EventSink, id: ConnectionId) -> Self {
        ConnectionSink(event_sink, id)
    }

    pub fn id(&self) -> ConnectionId {
        self.1
    }

    pub fn emit_incoming(&mut self, local: Addr, peer: Addr) {
        self.0.emit_event(MapiEvent::Incoming {
            id: self.id(),
            local,
            peer,
        });
    }

    pub fn emit_connecting(&mut self, remote: Addr) {
        self.0.emit_event(MapiEvent::Connecting {
            id: self.id(),
            remote,
        });
    }

    pub fn emit_connect_failed(&mut self, remote: String, immediately: bool, error: io::Error) {
        self.0.emit_event(MapiEvent::ConnectFailed {
            id: self.id(),
            remote,
            error,
            immediately,
        });
    }

    pub fn emit_connected(&mut self, remote: Addr) {
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

    pub fn emit_data(&mut self, direction: Direction, data: &[u8]) {
        self.0.emit_event(MapiEvent::Data {
            id: self.id(),
            direction,
            data: SmallVec::from_slice(data),
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
