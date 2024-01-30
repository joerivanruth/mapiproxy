use std::fmt;

use super::Error;

pub type ConnectionId = usize;

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

#[derive(Debug)]
pub enum MapiEvent {
    BoundPort(String),
    Incoming {
        id: ConnectionId,
        local: String,
        peer: String,
    },
    Connecting {
        id: ConnectionId,
        remote: String,
    },
    Connected {
        id: ConnectionId,
        peer: String,
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
        data: Vec<u8>,
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

    pub fn emit_bound(&mut self, port: String) {
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
