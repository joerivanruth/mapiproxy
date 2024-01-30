use core::fmt;
use std::{
    fmt::Display, io::{self, BufWriter, Write}, time::{Duration, Instant}
};

use crate::proxy::event::{ConnectionId, Direction};

pub struct Renderer {
    out: BufWriter<Box<dyn io::Write + 'static + Send>>,
    last_time: Option<Instant>,
}

impl Renderer {
    pub fn new(out: impl io::Write + 'static + Send) -> Self {
        let boxed: Box<dyn io::Write + 'static + Send> = Box::new(out);
        let buffered = BufWriter::with_capacity(4 * 8192, boxed);
        Renderer { out: buffered, last_time: None }
    }

    const THRESHOLD: Duration = Duration::from_millis(500);

    fn before(&mut self) -> io::Result<()> {
        if let Some(then) = self.last_time {
            let duration = then.elapsed();
            if duration >= Self::THRESHOLD {
                writeln!(self.out)?;
            }
        }
        Ok(())
    }
    
    fn after(&mut self) {
        self.last_time = Some(Instant::now());
    }

    pub fn message(
        &mut self,
        id: Option<ConnectionId>,
        direction: Option<Direction>,
        message: impl Display,
    ) -> io::Result<()> {
        self.before()?;
        writeln!(self.out, "‣{} {message}", IdStream::from((id, direction)))?;
        self.out.flush()?;
        self.after();
        Ok(())
    }

    pub fn header(&mut self, id: ConnectionId, direction: Direction, items: &[&dyn fmt::Display]) -> io::Result<()>
    {
        self.before()?;
        write!(self.out, "┌ {}", IdStream::from((id, direction)))?;
        let mut sep = " ";
        for item in items {
            write!(self.out, "{sep}{item}")?;
            sep = ", ";
        }
        writeln!(self.out)?;
        Ok(())
    }

    pub fn footer(&mut self, items: &[&dyn fmt::Display]) -> io::Result<()> {
        self.before()?;
        write!(self.out, "└")?;
        let mut sep = " ";
        for item in items {
            write!(self.out, "{sep}{item}")?;
            sep = ", ";
        }
        writeln!(self.out)?;
        self.out.flush()?;
        self.after();
        Ok(())
    }

    pub fn line(&mut self, item: &str) -> io::Result<()> {
        writeln!(self.out, "│{item}")
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
