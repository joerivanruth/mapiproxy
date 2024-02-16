use core::fmt;
use std::{
    fmt::Display,
    io::{self, BufWriter, Write},
    mem,
    time::{Duration, Instant},
};

use crate::proxy::event::{ConnectionId, Direction};

pub struct Renderer {
    colored: bool,
    last_time: Option<Instant>,
    out: BufWriter<Box<dyn io::Write + 'static + Send>>,
    current_style: Style,
    at_start: Option<Style>, // if Some(s), we're at line start, style to be reset to s
}

impl Renderer {
    pub fn new(colored: bool, out: impl io::Write + 'static + Send) -> Self {
        let boxed: Box<dyn io::Write + 'static + Send> = Box::new(out);
        let buffered = BufWriter::with_capacity(4 * 8192, boxed);
        Renderer {
            colored,
            out: buffered,
            current_style: Style::Normal,
            at_start: Some(Style::Normal),
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
        self.style(Style::Frame)?;
        writeln!(self.out, "‣{} {message}", IdStream::from((id, direction)))?;
        self.style(Style::Normal)?;
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
        let old_style = self.style(Style::Frame)?;
        write!(self.out, "┌{}", IdStream::from((id, direction)))?;
        let mut sep = " ";
        for item in items {
            write!(self.out, "{sep}{item}")?;
            sep = ", ";
        }
        writeln!(self.out)?;
        self.at_start = Some(old_style);
        assert_eq!(self.current_style, Style::Frame);
        Ok(())
    }

    pub fn footer(&mut self, items: &[&dyn fmt::Display]) -> io::Result<()> {
        self.clear_line()?;
        assert_eq!(self.current_style, Style::Frame);
        write!(self.out, "└")?;
        let mut sep = " ";
        for item in items {
            write!(self.out, "{sep}{item}")?;
            sep = ", ";
        }
        writeln!(self.out)?;
        self.style(Style::Normal)?;
        self.out.flush()?;
        self.after();
        Ok(())
    }

    pub fn put(&mut self, data: impl AsRef<[u8]>) -> io::Result<()> {
        if let Some(style) = self.at_start {
            assert_eq!(self.current_style, Style::Frame);
            self.out.write_all("│".as_bytes())?;
            self.style(style)?;
            self.at_start = None;
        }
        self.out.write_all(data.as_ref())?;
        Ok(())
    }

    pub fn clear_line(&mut self) -> io::Result<()> {
        if self.at_start.is_none() {
            self.nl()?;
        }
        Ok(())
    }

    pub fn nl(&mut self) -> io::Result<()> {
        let old_style = self.style(Style::Frame)?;
        writeln!(self.out)?;
        self.at_start = Some(old_style);
        Ok(())
    }

    pub fn style(&mut self, mut style: Style) -> io::Result<Style> {
        if style == self.current_style {
            return Ok(style);
        }
        if self.colored {
            self.write_style(style)?;
        }
        mem::swap(&mut self.current_style, &mut style);
        Ok(style)
    }

    fn write_style(&mut self, style: Style) -> io::Result<()> {
        let escape_sequence = match style {
            Style::Normal => "\u{1b}[m",
            Style::Header => "\u{1b}[1m",
            Style::Frame => "\u{1b}[36m",
            Style::Error => "\u{1b}[31m",
        };
        self.out.write_all(escape_sequence.as_bytes())?;
        Ok(())
    }
}

pub struct IdStream(Option<ConnectionId>, Option<Direction>);

impl fmt::Display for IdStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(id) = self.0 {
            write!(f, " {id}")?;
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
    Error,
    Frame,
    Header,
}
