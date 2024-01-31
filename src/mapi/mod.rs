mod analyzer;

use std::{collections::HashMap, io};

use crate::{
    proxy::event::{ConnectionId, Direction, MapiEvent},
    render::{Renderer, Style},
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
    force_binary: bool,
    analyzer: Analyzer,
    binary: Binary,
    buf: Vec<u8>,
}

impl Accumulator {
    fn new(id: ConnectionId, direction: Direction, level: Level, force_binary: bool) -> Self {
        Accumulator {
            id,
            direction,
            level,
            force_binary,
            analyzer: Analyzer::new(),
            binary: Binary::new(),
            buf: Vec::with_capacity(8192),
        }
    }

    fn handle_data(&mut self, data: &[u8], renderer: &mut Renderer) -> io::Result<()> {
        match self.level {
            Level::Raw => self.handle_raw(renderer, data),
            Level::Blocks | Level::Messages => self.handle_frame(renderer, data),
        }
    }

    fn handle_raw(&mut self, renderer: &mut Renderer, mut data: &[u8]) -> Result<(), io::Error> {
        renderer.header(
            self.id,
            self.direction,
            &[&format_args!("{n} bytes", n = data.len())],
        )?;
        while let Some(head) = self.analyzer.split_chunk(&mut data) {
            let is_head = self.analyzer.was_head();
            for b in head {
                self.binary.add(*b, is_head, renderer)?;
            }
        }
        self.binary.finish(renderer)?;
        renderer.footer(&[])?;
        Ok(())
    }

    fn handle_frame(&mut self, renderer: &mut Renderer, mut data: &[u8]) -> Result<(), io::Error> {
        while let Some(chunk) = self.analyzer.split_chunk(&mut data) {
            if self.analyzer.was_head() {
                continue;
            }

            let at_end = match self.level {
                Level::Blocks => self.analyzer.was_block_boundary(),
                Level::Messages => self.analyzer.was_message_boundary(),
                Level::Raw => unreachable!(),
            };

            if !at_end {
                self.buf.extend_from_slice(chunk);
                continue;
            }

            // we have a complete frame, dump it
            let frame = if self.buf.is_empty() {
                Some(chunk)
            } else {
                self.buf.extend_from_slice(chunk);
                None
            };
            self.dump_frame(frame, renderer)?;
            self.buf.clear();
        }
        Ok(())
    }

    fn dump_frame(&mut self, data: Option<&[u8]>, renderer: &mut Renderer) -> io::Result<()> {
        let data = data.unwrap_or(&self.buf);
        let len = data.len();
        let kind = match self.level {
            Level::Blocks => "block",
            Level::Messages => "message",
            Level::Raw => unreachable!(),
        };
        renderer.header(
            self.id,
            self.direction,
            &[&kind, &format_args!("{len} bytes")],
        )?;
        if self.force_binary || memchr::memchr(0u8, data).is_some() {
            self.dump_frame_as_binary(data, renderer)?;
        } else if let Ok(s) = std::str::from_utf8(data) {
            self.dump_frame_as_text(s, renderer)?;
        } else {
            self.dump_frame_as_binary(data, renderer)?;
        }
        renderer.footer(&[])?;
        Ok(())
    }

    fn check_incomplete(&mut self, _renderer: &mut Renderer) -> io::Result<()> {
        Ok(())
    }

    fn dump_frame_as_binary(&self, data: &[u8], renderer: &mut Renderer) -> io::Result<()> {
        let mut bin = Binary::new();
        for b in data {
            bin.add(*b, false, renderer)?;
        }
        bin.finish(renderer)
    }

    fn dump_frame_as_text(&self, text: &str, renderer: &mut Renderer) -> io::Result<()> {
        let data = text.as_bytes();
        for byte in data {
            match *byte {
                b'\n' => {
                    renderer.put("↵")?;
                    renderer.nl()?;
                }
                b'\t' => {
                    renderer.put("→")?;
                }
                b => renderer.put([b])?,
            }
        }
        renderer.clear_line()?;
        Ok(())
    }
}

#[derive(Debug)]
struct Binary {
    row: [(u8, bool); 16],
    col: usize,
}

impl Binary {
    fn new() -> Self {
        Binary {
            row: [(0, false); 16],
            col: 0,
        }
    }

    fn add(&mut self, byte: u8, is_head: bool, renderer: &mut Renderer) -> io::Result<()> {
        self.row[self.col] = (byte, is_head);
        self.col += 1;

        if self.col == 16 {
            self.write_out(renderer, false)
        } else {
            Ok(())
        }
    }

    fn finish(&mut self, renderer: &mut Renderer) -> io::Result<()> {
        if self.col == 0 {
            return Ok(());
        }
        self.write_out(renderer, true)
    }

    fn write_out(&mut self, renderer: &mut Renderer, _keep_head_state: bool) -> io::Result<()> {
        const HEX_DIGITS: [u8; 16] = *b"0123456789abcdef";
        let mut cur_head = false;
        for (i, (byte, is_head)) in self.row[..self.col].iter().cloned().enumerate() {
            self.put_sep(i, &mut cur_head, is_head, renderer)?;

            let hi = HEX_DIGITS[byte as usize / 16];
            let lo = HEX_DIGITS[byte as usize & 0xF];

            let style = if is_head {
                Style::Header
            } else {
                Style::Normal
            };
            renderer.style(style)?;
            renderer.put([hi, lo])?;
            renderer.style(Style::Normal)?;
        }

        for i in self.col..16 {
            self.put_sep(i, &mut cur_head, false, renderer)?;
            renderer.put(b"__")?;
        }

        // if the sep includes a style change, this is its
        // chance to wrap it up
        self.put_sep(16, &mut cur_head, false, renderer)?;

        for (byte, _) in &self.row[..self.col] {
            renderer.put(Self::readable(&[*byte]))?;
        }

        renderer.nl()?;

        self.col = 0;
        Ok(())
    }

    fn put_sep(
        &self,
        i: usize,
        in_head: &mut bool,
        is_head: bool,
        renderer: &mut Renderer,
    ) -> Result<(), io::Error> {
        let extra_space: [u8; 17] = [
            0, 0, 0, 0, //
            1, 0, 0, 0, //
            2, 0, 0, 0, //
            1, 0, 0, 0, //
            4,
        ];
        let spaces = "          ";
        let extra = extra_space[i] as usize;
        // let (open, close) = ("⟨", "⟩");
        let (open, close) = ("«", "»");
        match (*in_head, is_head) {
            (false, true) => {
                renderer.put(&spaces[..extra])?;
                renderer.put(open)?;
            }
            (true, false) => {
                renderer.put(close)?;
                renderer.put(&spaces[..extra])?;
            }
            _ => renderer.put(&spaces[..extra + 1])?,
        }
        *in_head = is_head;
        Ok(())
    }

    fn readable(byte: &[u8; 1]) -> &[u8] {
        let s = match byte[0] {
            b' '..=127 => return byte.as_ref(),
            b'\n' => "↵",
            b'\t' => "→",
            0 => "░",
            _ => "▒",
        };
        s.as_bytes()
    }
}
