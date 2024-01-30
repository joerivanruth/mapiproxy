mod analyzer;

use std::{collections::HashMap, fmt, io};

use crate::{
    proxy::event::{ConnectionId, Direction, MapiEvent},
    render::Renderer,
    Level,
};

use self::analyzer::Analyzer;

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
    _force_binary: bool,
    analyzer: Analyzer,
    binary: Binary,
    _buf: Vec<u8>,
}

impl Accumulator {
    fn new(id: ConnectionId, direction: Direction, level: Level, force_binary: bool) -> Self {
        Accumulator {
            id,
            direction,
            level,
            _force_binary: force_binary,
            analyzer: Analyzer::new(),
            binary: Binary::new(),
            _buf: Vec::with_capacity(8192),
        }
    }

    fn handle_data(&mut self, mut data: &[u8], renderer: &mut Renderer) -> io::Result<()> {
        assert_eq!(self.level, Level::Raw);

        renderer.header(self.id, self.direction, &[&format_args!("{n} bytes", n=data.len())])?;
        while let Some((head, tail)) = self.analyzer.split_chunk(data) {
            data = tail;
            for b in head {
                if let Some(s) = self.binary.add(*b) {
                    renderer.line(s)?;
                }
            }
        }
        if let Some(s) = self.binary.finish() {
            renderer.line(s)?;
        }
        renderer.footer(&[])?;

        Ok(())
    }

    fn _handle_datax(&mut self, mut data: &[u8], renderer: &mut Renderer) -> io::Result<()> {
        let mut render = |msg: &dyn fmt::Display| -> io::Result<()> {
            renderer.message(Some(self.id), Some(self.direction), msg)
        };
        while let Some((head, tail)) = self.analyzer.split_chunk(data) {
            data = tail;
            let kind = if self.analyzer.was_head() {
                "head"
            } else {
                "body"
            };
            render(&format_args!("{kind} {head:?}"))?;
            if self.analyzer.was_block_boundary() {
                render(&"block boundary")?;
            }
            if self.analyzer.was_message_boundary() {
                render(&"message boundary")?;
            }
        }

        Ok(())
    }

    fn check_incomplete(&mut self, _renderer: &mut Renderer) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct Binary {
    buf: String,
    text: String,
    col: usize,
}

static HEXDIGITS: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
];

impl Binary {
    fn new() -> Self {
        Binary {
            buf: String::with_capacity(128),
            text: String::with_capacity(16),
            col: 0,
        }
    }

    fn add(&mut self, byte: u8) -> Option<&str> {
        if self.col == 0 {
            self.buf.clear();
            self.text.clear();
        }
        self.buf.push(HEXDIGITS[(byte / 16) as usize]);
        self.buf.push(HEXDIGITS[(byte & 0xF) as usize]);
        self.text.push(Self::readable(byte));

        self.col += 1;
        self.add_sep();

        if self.col == 16 {
            Some(self.complete())
        } else {
            None
        }
    }

    fn finish(&mut self) -> Option<&str> {
        if self.col == 0 {
            return None;
        }
        while self.col < 16 {
            self.buf.push_str("__");
            self.col += 1;
            self.add_sep();
        }
        let s = self.complete();
        Some(s)
    }

    fn add_sep(&mut self) {
        self.buf.push(' ');
        if self.col % 4 == 0 {
            self.buf.push(' ');
        }
        if self.col % 8 == 0 {
            self.buf.push(' ');
        }
    }

    fn complete(&mut self) -> &str {
        assert_eq!(self.col, 16);
        self.col = 0;
        self.buf.push_str("    ");
        self.buf.push_str(&self.text);
        &self.buf
    }

    fn readable(byte: u8) -> char {
        match byte {
            b' '..=127 => unsafe { char::from_u32_unchecked(byte as u32) },
            b'\n' => '↵',
            b'\t' => '→',
            _ => '⋄',
        }
    }
}
