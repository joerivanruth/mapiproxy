use std::{fmt, io};

use smallvec::SmallVec;

use super::{network::Addr, Error};

/// Connection id for display to the user.
/// Displayed with a leading #, e.g., #10.
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

/// Enum to indicate client->server versus server->client
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Direction {
    /// Traffic flowing from client to server
    Upstream,
    /// Traffic flowing from server to client
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

    /// Return 'client' or 'server' depending on where traffic comes from
    pub fn sender(&self) -> &'static str {
        match self {
            Direction::Upstream => Self::CLIENT,
            Direction::Downstream => Self::SERVER,
        }
    }

     /// Return 'client' or 'server' depending on where traffic goes
     pub fn receiver(&self) -> &'static str {
        match self {
            Direction::Upstream => Self::SERVER,
            Direction::Downstream => Self::CLIENT,
        }
    }
}

/// Type to represent the events that need to be reported on
#[derive(Debug)]
pub enum MapiEvent {
    /// Proxy has succesfully bound listen port
    BoundPort(Addr),

    /// A new client connection has been detected. Introduces a newly allocated
    /// [ConnectionId].
    Incoming {
        id: ConnectionId,
        local: Addr,
        peer: Addr,
    },

    /// Proxy is connecting to the server
    Connecting {
        id: ConnectionId,
        remote: Addr,
    },

    /// Server has accepted the new connection
    Connected {
        id: ConnectionId,
        peer: Addr,
    },

    /// The connection has ended peacefully, no more events on this
    /// [ConnectionId] will be reported.
    End {
        id: ConnectionId,
    },

    /// Something went wrong in Mapiproxy (not in the client or the server), no
    /// more events on this [ConnectionId] will be reported.
    Aborted {
        id: ConnectionId,
        error: Error,
    },

    /// Data has been observed flowing from client to server
    /// ([Direction::Upstream]) or from server to client
    /// ([Direction::Downstream]).
    Data {
        id: ConnectionId,
        direction: Direction,
        data: SmallVec<[u8; 8]>,
    },

    /// Client or server has shut down the write-half of its socket. No more data will
    /// flow in this direction.
    ShutdownRead {
        id: ConnectionId,
        direction: Direction,
    },

    /// Client or server has shut down the read-half of its socket. No more data
    /// flowing in this direction will be accepted. This event includes the number
    /// of bytes that are still inside the proxy and can no longer be sent onward.
    ShutdownWrite {
        id: ConnectionId,
        direction: Direction,
        discard: usize,
    },

    /// The connection attempt from proxy to server has failed. The proxy
    /// uses non-blocking I/O. If the attempt was refused immediately, for
    /// example because the address is bad, field `immediately` will be `true`.
    /// If the attempt failed later, for example because the server refused
    /// the connection, it will be `false`.
    ConnectFailed {
        id: ConnectionId,
        remote: String,
        error: io::Error,
        immediately: bool,
    },
}

/// Struct [EventSink] knows what to do with new [MapiEvent]s and
/// provides helper functions to generate such events.
///
/// Method [connection_sink] returns a derived struct that also holds
/// a connection id and is used to emit events specific to a single
/// connection.
pub struct EventSink(Box<dyn FnMut(MapiEvent) + 'static + Send>);

impl EventSink {
    /// Create a new EventSink, wrapping a function that will deliver the events
    /// somehow.
    pub fn new(f: impl FnMut(MapiEvent) + 'static + Send) -> Self {
        EventSink(Box::new(f))
    }

    /// Create a [ConnectionSink] that will deliver messages about a specific
    /// connection id.
    pub fn connection_sink(&mut self, id: ConnectionId) -> ConnectionSink<'_> {
        ConnectionSink::new(&mut *self, id)
    }

    /// Emit the given event.
    fn emit_event(&mut self, event: MapiEvent) {
        (self.0)(event)
    }

    /// Emit a [MapiEvent::BoundPort] event.
    pub fn emit_bound(&mut self, port: Addr) {
        self.emit_event(MapiEvent::BoundPort(port))
    }
}

/// Helper struct to emit [MapiEvent]s about a specific connection.
pub struct ConnectionSink<'a>(&'a mut EventSink, ConnectionId);

impl<'a> ConnectionSink<'a> {
    pub fn new(event_sink: &'a mut EventSink, id: ConnectionId) -> Self {
        ConnectionSink(event_sink, id)
    }

    /// The [ConnectionId] this struct emits events about
    pub fn id(&self) -> ConnectionId {
        self.1
    }

    /// Emit a [MapiEvent::Incoming] event.
    pub fn emit_incoming(&mut self, local: Addr, peer: Addr) {
        self.0.emit_event(MapiEvent::Incoming {
            id: self.id(),
            local,
            peer,
        });
    }

    /// Emit a [MapiEvent::Connecting] event.
    pub fn emit_connecting(&mut self, remote: Addr) {
        self.0.emit_event(MapiEvent::Connecting {
            id: self.id(),
            remote,
        });
    }

    /// Emit a [MapiEvent::ConnectFailed] event.
    pub fn emit_connect_failed(&mut self, remote: String, immediately: bool, error: io::Error) {
        self.0.emit_event(MapiEvent::ConnectFailed {
            id: self.id(),
            remote,
            error,
            immediately,
        });
    }

    /// Emit a [MapiEvent::Connected] event.
    pub fn emit_connected(&mut self, remote: Addr) {
        self.0.emit_event(MapiEvent::Connected {
            id: self.id(),
            peer: remote,
        });
    }

    /// Emit a [MapiEvent::End] event.
    pub fn emit_end(&mut self) {
        self.0.emit_event(MapiEvent::End { id: self.id() });
    }

    /// Emit a [MapiEvent::Aborted] event.
    pub fn emit_aborted(&mut self, error: Error) {
        self.0.emit_event(MapiEvent::Aborted {
            id: self.id(),
            error,
        });
    }

    /// Emit a [MapiEvent::Data] event.
    pub fn emit_data(&mut self, direction: Direction, data: &[u8]) {
        self.0.emit_event(MapiEvent::Data {
            id: self.id(),
            direction,
            data: SmallVec::from_slice(data),
        })
    }

    /// Emit a [MapiEvent::ShutdownRead] event.
    pub fn emit_shutdown_read(&mut self, direction: Direction) {
        self.0.emit_event(MapiEvent::ShutdownRead {
            id: self.id(),
            direction,
        });
    }

    /// Emit a [MapiEvent::ShutdownWrite] event.
    pub fn emit_shutdown_write(&mut self, direction: Direction, discard: usize) {
        self.0.emit_event(MapiEvent::ShutdownWrite {
            id: self.id(),
            direction,
            discard,
        });
    }
}
