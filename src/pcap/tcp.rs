#![allow(dead_code, unused_variables)]

use std::{
    collections::HashMap,
    io,
    net::{IpAddr, SocketAddr as TcpSocketAddr},
    ops::RangeFrom,
};

use etherparse::TcpSlice;

use crate::proxy::event::{ConnectionId, Direction, MapiEvent};

type Handler<'a> = dyn FnMut(MapiEvent) -> io::Result<()> + 'a;

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
struct Key {
    src: TcpSocketAddr,
    dest: TcpSocketAddr,
}

impl Key {
    fn flip(&self) -> Self {
        Key {
            src: self.dest,
            dest: self.src,
        }
    }
}

pub struct TcpTracker {
    conn_ids: RangeFrom<usize>,
    streams: HashMap<Key, StreamState>,
}

impl TcpTracker {
    pub fn new() -> Self {
        TcpTracker {
            conn_ids: 10..,
            streams: Default::default(),
        }
    }

    pub fn handle(
        &mut self,
        src_addr: IpAddr,
        dest_addr: IpAddr,
        tcp: &TcpSlice,
        handler: &mut Handler,
    ) -> io::Result<()> {
        let key = Key {
            src: (src_addr, tcp.source_port()).into(),
            dest: (dest_addr, tcp.destination_port()).into(),
        };

        match (tcp.syn(), tcp.ack()) {
            (true, false) => self.handle_syn(key, tcp, handler),
            (true, true) => self.handle_syn_ack(key, tcp, handler),
            _ => self.handle_existing(key, tcp, handler),
        }
    }

    fn handle_syn(&mut self, key: Key, tcp: &TcpSlice, handler: &mut Handler) -> io::Result<()> {
        let flipped = key.flip();
        if self.streams.contains_key(&key) || self.streams.contains_key(&flipped) {
            return Ok(());
        }

        let id = ConnectionId::new(self.conn_ids.next().unwrap());
        let upstream = StreamState {
            id,
            dir: Direction::Upstream,
            finished: false,
        };

        let ev = MapiEvent::Incoming {
            id,
            local: key.dest.into(),
            peer: key.src.into(),
        };
        handler(ev)?;

        self.streams.insert(key, upstream);
        Ok(())
    }

    fn handle_syn_ack(
        &mut self,
        key: Key,
        tcp: &TcpSlice,
        handler: &mut Handler,
    ) -> io::Result<()> {
        let flipped = key.flip();
        let Some(upstream) = self.streams.get(&flipped) else {
            return Ok(());
        };
        let id = upstream.id;
        let downstream = StreamState {
            id,
            dir: Direction::Downstream,
            finished: false,
        };

        let ev = MapiEvent::Connected {
            id,
            peer: key.src.into(),
        };
        handler(ev)?;

        self.streams.insert(key, downstream);
        Ok(())
    }

    fn handle_existing(
        &mut self,
        key: Key,
        tcp: &TcpSlice,
        handler: &mut Handler,
    ) -> io::Result<()> {
        let Some(stream) = self.streams.get_mut(&key) else {
            return Ok(());
        };

        let id = stream.id;
        let direction = stream.dir;

        let payload = tcp.payload();
        if !payload.is_empty() {
            let ev = MapiEvent::Data {
                id,
                direction,
                data: payload.into(),
            };
            handler(ev)?;
        }

        if !tcp.fin() {
            return Ok(());
        }
        stream.finished = true;

        let ev = MapiEvent::ShutdownRead { id, direction };
        handler(ev)?;

        let flipped = key.flip();
        if let Some(StreamState { finished: true, .. }) = self.streams.get(&flipped) {
            self.streams.remove(&key);
            self.streams.remove(&flipped);
            let ev = MapiEvent::End { id };
            handler(ev)?;
        }

        Ok(())
    }
}

#[derive(Debug)]
struct StreamState {
    id: ConnectionId,
    dir: Direction,
    finished: bool,
}
