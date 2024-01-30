use core::fmt;
use std::{
    fmt::Display,
    io::{self, BufWriter, Write},
};

use crate::proxy::event::{ConnectionId, Direction};

pub struct Renderer {
    out: BufWriter<Box<dyn io::Write + 'static + Send>>,
}

impl Renderer {
    pub fn new(out: impl io::Write + 'static + Send) -> Self {
        let boxed: Box<dyn io::Write + 'static + Send> = Box::new(out);
        let buffered = BufWriter::with_capacity(4 * 8192, boxed);
        Renderer { out: buffered }
    }

    pub fn message(
        &mut self,
        id: Option<ConnectionId>,
        direction: Option<Direction>,
        message: impl Display,
    ) -> io::Result<()> {
        let message = message;
        writeln!(self.out, "‣{} {message}", IdStream::from((id, direction)))?;
        self.out.flush()
    }
}

pub struct IdStream(Option<ConnectionId>, Option<Direction>);

impl fmt::Display for IdStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(id) = self.0 {
            write!(f, " [{id}]")?;
        }
        if let Some(dir) = self.1 {
            write!(f, " {dir}")?;
        }
        Ok(())
    }
}

impl From<(ConnectionId, Direction)> for IdStream {
    fn from(value: (ConnectionId, Direction)) -> Self {
        let (id, dir) = value;
        IdStream(Some(id), Some(dir))
    }
}

impl From<(Option<ConnectionId>, Option<Direction>)> for IdStream {
    fn from(value: (Option<ConnectionId>, Option<Direction>)) -> Self {
        let (id, dir) = value;
        IdStream(id, dir)
    }
}
