use core::fmt;
use std::{
    fmt::Display,
    io::{self, BufWriter, Write},
    mem,
    time::{Duration, Instant},
};

use crate::proxy::event::{ConnectionId, Direction};

pub struct Renderer {
    last_time: Option<Instant>,
    out: BufWriter<Box<dyn io::Write + 'static + Send>>,
    at_start: bool,
    style: Style,
}

impl Renderer {
    pub fn new(out: impl io::Write + 'static + Send) -> Self {
        let boxed: Box<dyn io::Write + 'static + Send> = Box::new(out);
        let buffered = BufWriter::with_capacity(4 * 8192, boxed);
        Renderer {
            out: buffered,
            at_start: true,
            style: Style::Normal,
            last_time: None,
        }
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

    pub fn header(
        &mut self,
        id: ConnectionId,
        direction: Direction,
        items: &[&dyn fmt::Display],
    ) -> io::Result<()> {
        self.before()?;
        write!(self.out, "┌ {}", IdStream::from((id, direction)))?;
        let mut sep = " ";
        for item in items {
            write!(self.out, "{sep}{item}")?;
            sep = ", ";
        }
        writeln!(self.out)?;
        self.at_start = true;
        assert_eq!(self.style, Style::Normal);
        Ok(())
    }

    pub fn footer(&mut self, items: &[&dyn fmt::Display]) -> io::Result<()> {
        self.clear_line()?;
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

    pub fn put(&mut self, data: impl AsRef<[u8]>) -> io::Result<()> {
        if self.at_start {
            self.out.write_all("│".as_bytes())?;
            if self.style != Style::Normal {
                self.write_style(self.style)?;
            }
            self.at_start = false;
        }
        self.out.write_all(data.as_ref())?;
        Ok(())
    }

    pub fn clear_line(&mut self) -> io::Result<()> {
        if !self.at_start {
            self.nl()?;
        }
        Ok(())
    }

    pub fn nl(&mut self) -> io::Result<()> {
        if self.style != Style::Normal {
            self.write_style(Style::Normal)?;
        }
        writeln!(self.out)?;
        self.at_start = true;
        Ok(())
    }

    pub fn style(&mut self, mut style: Style) -> io::Result<Style> {
        if style == self.style {
            return Ok(style);
        }
        if !self.at_start {
            self.write_style(style)?;
        }
        mem::swap(&mut self.style, &mut style);
        Ok(style)
    }

    fn write_style(&mut self, style: Style) -> io::Result<()> {
        let escapes = match style {
            Style::Normal => "\u{1b}[m",
            // Style::Header => "\u{1b}[42m",
            // Style::Header => "\u{1b}[1m",
            // Style::Header => "\u{1b}[2m",
            // Style::Header => "\u{1b}[4m",
            // Style::Header => "\u{1b}[32m\u{1b}[4m",
            // Style::Header => "\u{1b}[36m\u{1b}[2m",
            Style::Header => "\u{1b}[32m\u{1b}[1m",
        };
        self.out.write_all(escapes.as_bytes())?;
        Ok(())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Style {
    Normal,
    Header,
}
