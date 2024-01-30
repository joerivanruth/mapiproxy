use std::{
    collections::HashMap,
    io::{self, ErrorKind},
};

use either::Either;

use crate::{
    proxy::event::{ConnectionId, Direction, MapiEvent},
    render::Renderer,
    Level,
    Level::*,
};

#[derive(Debug)]
pub struct State {
    level: Level,
    force_binary: bool,
    accs: HashMap<usize, (Accumulator, Accumulator)>,
}

impl State {
    pub fn new(level: Level, force_binary: bool) -> Self {
        State {
            level,
            force_binary,
            accs: Default::default(),
        }
    }

    pub fn handle(&mut self, event: &MapiEvent, renderer: &mut Renderer) -> io::Result<()> {
        match event {
            MapiEvent::BoundPort(port) => {
                renderer.message(None, None, format_args!("LISTEN on port {port}"))?;
            }

            MapiEvent::Incoming { id, local, peer } => {
                renderer.message(
                    Some(*id),
                    None,
                    format_args!("INCOMING on {local} from {peer}"),
                )?;
                self.add_connection(id);
            }

            MapiEvent::Connecting { id, remote } => {
                renderer.message(Some(*id), None, format_args!("CONNECTING to {remote}"))?;
            }

            MapiEvent::Connected { id, .. } => {
                renderer.message(Some(*id), None, "CONNECTED")?;
            }

            MapiEvent::End { id } => {
                renderer.message(Some(*id), None, "ENDED")?;
                self.check_incomplete(*id, Direction::Upstream, renderer)?;
                self.check_incomplete(*id, Direction::Downstream, renderer)?;
                self.remove_connection(id);
            }

            MapiEvent::Aborted { id, error } => {
                renderer.message(Some(*id), None, format_args!("ABORTED: {error}"))?;
                self.check_incomplete(*id, Direction::Upstream, renderer)?;
                self.check_incomplete(*id, Direction::Downstream, renderer)?;
                self.remove_connection(id);
            }

            MapiEvent::Data {
                id,
                direction,
                data,
            } => {
                let Some((upstream, downstream)) = self.accs.get_mut(id) else {
                    panic!("got data for conn {id} but don't have accumulators for it")
                };
                let acc = match direction {
                    Direction::Upstream => upstream,
                    Direction::Downstream => downstream,
                };
                acc.handle_data(data, renderer)?;
            }

            MapiEvent::ShutdownRead { id, direction } => {
                self.check_incomplete(*id, *direction, renderer)?;
                renderer.message(Some(*id), Some(*direction), "shut down reading")?;
            }

            MapiEvent::ShutdownWrite {
                id,
                direction,
                discard: 0,
            } => {
                renderer.message(Some(*id), Some(*direction), "shut down writing")?;
            }

            MapiEvent::ShutdownWrite {
                id,
                direction,
                discard: n,
            } => {
                renderer.message(
                    Some(*id),
                    Some(*direction),
                    format_args!("shut down writing, discarded {n} bytes"),
                )?;
            }
        }

        Ok(())
    }

    fn add_connection(&mut self, id: &usize) {
        let level = self.level;
        let upstream = Accumulator::new(*id, Direction::Upstream, level, self.force_binary);
        let downstream = Accumulator::new(*id, Direction::Downstream, level, self.force_binary);
        let new = (upstream, downstream);
        let prev = self.accs.insert(*id, new);
        if prev.is_some() {
            panic!("Already have state for incoming connection {id}");
        }
    }

    fn remove_connection(&mut self, id: &usize) {
        let ended = self.accs.remove(id);
        if ended.is_none() {
            panic!("Found no state to remove for end event on connection {id}");
        }
    }

    fn check_incomplete(
        &mut self,
        id: usize,
        direction: Direction,
        renderer: &mut Renderer,
    ) -> io::Result<()> {
        let Some((upstream, downstream)) = self.accs.get_mut(&id) else {
            panic!("got data for conn {id} but don't have accumulators for it")
        };
        let acc = match direction {
            Direction::Upstream => upstream,
            Direction::Downstream => downstream,
        };
        acc.check_incomplete(renderer)
    }
}

#[derive(Debug)]
pub struct Accumulator {
    id: ConnectionId,
    direction: Direction,
    level: Level,
    force_binary: bool,
    buf: Vec<u8>,
    header: Vec<u8>,
    footer: Vec<u8>,
    must_read: MustRead,
    last: bool,
}

impl Accumulator {
    fn new(id: ConnectionId, direction: Direction, level: Level, force_binary: bool) -> Self {
        Accumulator {
            id,
            direction,
            level,
            force_binary,
            buf: Vec::with_capacity(8192),
            header: Vec::with_capacity(80),
            footer: Vec::with_capacity(80),
            must_read: MustRead::Head,
            last: true,
        }
    }

    fn handle_data(&mut self, mut data: &[u8], renderer: &mut Renderer) -> io::Result<()> {
        let _orig = data.len();
        eprintln!("{dir:?} {data:?}", dir = self.direction);
        // eprintln!("Handling {orig} bytes ({dir:?})", dir = self.direction);
        eprint!("");
        while !data.is_empty() {
            // eprintln!(
            //     "At {n}/{orig}: {s:?} last={last}",
            //     n = orig - data.len(),
            //     s = self.must_read,
            //     last = self.last
            // );
            let n = self.accept_some_data(data, renderer)?;
            data = &data[n..];
        }
        // eprintln!(
        //     "At {orig}/{orig}: {s:?} last={last:?}",
        //     s = self.must_read,
        //     last = self.last
        // );
        Ok(())
    }

    fn check_incomplete(&mut self, _renderer: &mut Renderer) -> io::Result<()> {
        Ok(())
    }

    fn accept_some_data(&mut self, data: &[u8], renderer: &mut Renderer) -> io::Result<usize> {
        use MustRead::*;

        if self.level == Raw {
            self.render_body(Some(data), renderer)?;
            return Ok(data.len());
        }

        let len = data.len();
        let (consumed, new_state) = match (self.must_read, data) {
            (_, []) => unreachable!("this should only be called when there is some data"),

            (Head, [byte1, byte2, ..]) => (2, self.process_header(*byte1, *byte2)?),

            (Head, [byte1]) => (1, MustRead::PartialHead(*byte1)),

            (PartialHead(byte1), [byte2, ..]) => (1, self.process_header(byte1, *byte2)?),

            (Body(n), _) if self.level == Blocks && self.buf.is_empty() && data.len() >= n => {
                // no need to append to buffer first, we have it all right here
                self.render_body(Some(&data[..n]), renderer)?;
                (n, Head)
            }

            (Body(n), _) if len >= n => {
                self.buf.extend_from_slice(&data[..n]);
                match (self.level, self.last) {
                    (Blocks, _) | (Messages, true) => {
                        self.render_body(None, renderer)?;
                        self.buf.clear();
                    }
                    (Messages, false) => {
                        // not done yet, accumulate more
                    }
                    (Raw, _) => unreachable!("Raw has been checked above"),
                }
                (n, Head)
            }

            (Body(n), _) => {
                assert!(len < n);
                self.buf.extend_from_slice(data);
                (len, Body(n - len))
            }
        };

        self.must_read = new_state;
        Ok(consumed)
    }

    fn process_header(&mut self, byte1: u8, byte2: u8) -> io::Result<MustRead> {
        // Note: little endian!
        let n = byte2 as u16 * 256 + byte1 as u16;
        let max = 2 * 8190 + 1;
        if n > max {
            let msg = format!("Maximum header size is {max} ({max:x}), found {n} ({n:x}");
            let err = io::Error::new(ErrorKind::Other, msg);
            return Err(err);
        }
        let size = n / 2;
        self.last = n & 1 > 0;

        if size == 0 && !self.last {
            Ok(MustRead::Head)
        } else {
            Ok(MustRead::Body(size as usize))
        }
    }

    fn render_body(&mut self, body: Option<&[u8]>, renderer: &mut Renderer) -> io::Result<()> {
        let body = body.unwrap_or(&self.buf);
        // eprintln!("  {body:?}");
        let choice: Either<&str, &[u8]> = if self.force_binary {
            Either::Right(body)
        } else if memchr::memchr(0u8, body).is_some() {
            Either::Right(body)
        } else {
            match std::str::from_utf8(body) {
                Ok(s) => Either::Left(s),
                Err(_) => Either::Right(body),
            }
        };

        let id = Some(self.id);
        let dir = Some(self.direction);

        self.header.clear();
        self.footer.clear();
        match choice {
            Either::Left(s) => 
                renderer.message(id, dir, format_args!("text: {s}")),
            Either::Right(b) => 
                renderer.message(id, dir, format_args!("bin:  {b:?}")),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum MustRead {
    Head,
    PartialHead(u8),
    Body(usize),
}
