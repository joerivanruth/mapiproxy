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
    _level: Level,
    _force_binary: bool,
    analyzer: Analyzer,
    _buf: Vec<u8>,
}

impl Accumulator {
    fn new(id: ConnectionId, direction: Direction, level: Level, force_binary: bool) -> Self {
        Accumulator {
            id,
            direction,
            _level: level,
            _force_binary: force_binary,
            analyzer: Analyzer::new(),
            _buf: Vec::with_capacity(8192),
        }
    }

    fn handle_data(&mut self, mut data: &[u8], renderer: &mut Renderer) -> io::Result<()> {
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
