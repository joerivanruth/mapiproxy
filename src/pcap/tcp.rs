use std::{
    collections::HashMap,
    io,
    net::{IpAddr, SocketAddr as TcpSocketAddr},
    ops::RangeFrom,
};

use etherparse::TcpSlice;

use crate::proxy::event::{ConnectionId, Direction, MapiEvent};

type Handler<'a> = dyn FnMut(MapiEvent) -> io::Result<()> + 'a;

/// TCP connection state is identified by (src_ip,src_port, dest_ip,dest_port) tuples.
/// This struct represents those.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
struct Key {
    src: TcpSocketAddr,
    dest: TcpSocketAddr,
}

impl Key {
    /// Return src and dest swapped.
    fn flip(&self) -> Self {
        Key {
            src: self.dest,
            dest: self.src,
        }
    }
}

/// Keep track of all TCP connection state. For each connection we store
/// two [StreamState] entries.  One keyed by the TCP connection's [Key]
/// and one by its flipped ([Key::flip]) key.
pub struct TcpTracker {
    /// Iterator that yields fresh connection id's.
    conn_ids: RangeFrom<usize>,
    /// Container for the [StreamState]s. Once the connection is fully established,
    /// both its [Key] and its flipped ([Key::flip]) key will have an entry.
    streams: HashMap<Key, StreamState>,
}

impl TcpTracker {
    /// Create a new, empty, TCP tracker.
    pub fn new() -> Self {
        TcpTracker {
            conn_ids: 10..,
            streams: Default::default(),
        }
    }

    /// Handle a TCP packet.
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

        let seqno = tcp.sequence_number();

        let id = ConnectionId::new(self.conn_ids.next().unwrap());
        let upstream = StreamState::new(id, Direction::Upstream, seqno.wrapping_add(1));

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

        let seqno = tcp.sequence_number();

        let id = upstream.id;
        let downstream = StreamState::new(id, Direction::Downstream, seqno.wrapping_add(1));

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

        let seqno = tcp.sequence_number();
        let payload = tcp.payload();
        // Packets may arrive in the wrong order.
        // If this is exactly the packet we're waiting for, stream.reorder will
        // return it. If it's a future packet, it will store it.
        // If it's a past packet, it will drop it.
        let Some(payload) = stream.reorder(seqno, tcp.fin(), payload) else {
            return Ok(());
        };
        Self::emit_data(id, direction, payload, handler)?;

        // If stream.reorder above returned this packet, it means it was exactly
        // the packet we needed right now. Packets do not always arrive in-order
        // so it's possible that the next packet is already in our cache.
        while let Some(payload) = stream.next_ready() {
            Self::emit_data(id, direction, &payload, handler)?;
        }

        // Stream.finished is set by stream.reorder and stream.next_ready.
        if !stream.finished {
            return Ok(());
        }

        // This was the last packet of this direction of the TCP connection.
        // Report this and drop all state if the other direction has also finished.

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

    fn emit_data(
        id: ConnectionId,
        direction: Direction,
        payload: &[u8],
        handler: &mut Handler,
    ) -> io::Result<()> {
        if !payload.is_empty() {
            let ev = MapiEvent::Data {
                id,
                direction,
                data: payload.into(),
            };
            handler(ev)?;
        }
        Ok(())
    }
}

/// State stored for each half (client to server and server to client) of
/// each TCP connection.
#[derive(Debug)]
struct StreamState {
    /// Id to display for this connection
    id: ConnectionId,
    /// Is it upstream traffix (client to server) or downstream (server to client)
    dir: Direction,
    /// Sequence number of the next byte we hope to receive.
    waiting_for: u32,
    /// Packets with sequence numbers higher than [Self::waiting_for] we have
    /// already received.
    waiting: HashMap<u32, (Vec<u8>, bool)>,
    /// If no more packets will arrive
    finished: bool,
}

impl StreamState {
    /// Create a new [StreamState]
    fn new(id: ConnectionId, dir: Direction, seqno: u32) -> Self {
        StreamState {
            id,
            dir,
            waiting_for: seqno,
            waiting: Default::default(),
            finished: false,
        }
    }

    /// Check for duplicate packets and packets that arrive in the wrong order
    /// based on the sequence number. If this is exactly the sequence number we
    /// were waiting for, return it. If we've already processed this sequence
    /// number, drop it and return None. If it's a higher sequence number, store
    /// it in the map for later and also return None.
    ///
    /// When this function returns Some, [Self::next_ready] MUST be called next to
    /// retrieve any stored 'future' packets that can now be processed.
    fn reorder<'a>(&'a mut self, seqno: u32, fin: bool, payload: &'a [u8]) -> Option<&'a [u8]> {
        if self.waiting_for == seqno {
            return self.yield_payload(payload, fin);
        }

        // Discard packets we've already seen. Be careful with wraparound.
        // Example values: waiting_for = 0x30, seqno_1 = 0x31, seqno_2 = 0x2f.
        // delta_1 = 0x01, delta_2 = 0xff.
        // delta_1 as i32 = 1, delta_2 as i32 = -1
        let delta = seqno.wrapping_sub(self.waiting_for);
        if (delta as i32) < 0 {
            return None;
        }

        self.waiting.insert(seqno, (payload.to_owned(), fin));
        None
    }

    /// If the sequence number we're waiting for already exists in the map, return it.
    /// Call this repeatedly when [Self::reorder] has returned Some.
    fn next_ready(&mut self) -> Option<Vec<u8>> {
        if let Some((payload, fin)) = self.waiting.remove(&self.waiting_for) {
            self.yield_payload(payload, fin)
        } else {
            None
        }
    }

    /// Update the bookkeeping before returning the packet.
    fn yield_payload<T: AsRef<[u8]>>(&mut self, payload: T, fin: bool) -> Option<T> {
        self.finished |= fin;
        let n = payload.as_ref().len() as u32;
        self.waiting_for = self.waiting_for.wrapping_add(n);
        Some(payload)
    }
}
