use io::Error as IOError;
use io::ErrorKind as Kind;
use std::borrow::Cow;
use std::ffi::OsString;
use std::fmt;
use std::future::Future;
use std::net::Shutdown;
use std::net::TcpListener;
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::os::fd::BorrowedFd;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::result;
use std::{ffi::OsStr, io, net::ToSocketAddrs, path::PathBuf};

fn explain_io<T>(descr: &dyn fmt::Display, f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    match f() {
        Ok(c) => Ok(c),
        Err(e) => {
            let kind = e.kind();
            let message = format!("{descr}: {e}");
            Err(io::Error::new(kind, message))
        }
    }
}

pub enum Addr {
    Tcp(String),
    #[cfg(unix)]
    Unix(PathBuf),
}

impl TryFrom<&OsStr> for Addr {
    type Error = io::Error;

    fn try_from(value: &OsStr) -> Result<Self, Self::Error> {
        let bytes = value.as_encoded_bytes();
        if bytes.contains(&b'/') || bytes.contains(&b'\\') {
            // Must be Unix
            #[cfg(unix)]
            {
                let path = PathBuf::from(value);
                return Ok(Addr::Unix(path));
            }
            #[cfg(not(unix))]
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Unix domain sockets not supported",
                ));
            }
        }

        let text = match value.to_string_lossy() {
            Cow::Owned(lossy) => {
                return Err(IOError::new(
                    Kind::InvalidInput,
                    format!("Invalid unicode in addr '{lossy}'"),
                ))
            }
            Cow::Borrowed(s) => s,
        };

        let full = match text.parse::<u16>() {
            Ok(n) => format!("localhost:{n}"),
            Err(_) => text.to_owned(),
        };

        Ok(Addr::Tcp(full))
    }
}

impl TryFrom<OsString> for Addr {
    type Error = io::Error;

    fn try_from(value: OsString) -> Result<Self, Self::Error> {
        Self::try_from(value.as_os_str())
    }
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Addr::Tcp(addr) => addr.fmt(f),
            #[cfg(unix)]
            Addr::Unix(path) => path.display().fmt(f),
        }
    }
}

impl Addr {
    pub fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp(_))
    }

    pub fn is_unix(&self) -> bool {
        !self.is_tcp()
    }

    pub fn connect(&self) -> io::Result<(ReadHalf, WriteHalf)> {
        explain_io(self, || match self {
            Addr::Tcp(addr) => {
                let conn1 = TcpStream::connect(addr)?;
                let conn2 = conn1.try_clone()?;
                Ok((ReadHalf::Tcp(conn1), WriteHalf::Tcp(conn2)))
            }
            #[cfg(unix)]
            Addr::Unix(path) => {
                let conn1 = UnixStream::connect(path)?;
                let conn2 = conn1.try_clone()?;
                Ok((ReadHalf::Unix(conn1), WriteHalf::Unix(conn2)))
            }
        })
    }

    pub fn listen(&self) -> io::Result<Listener> {
        explain_io(self, || match self {
            Addr::Tcp(addr) => {
                let listener = TcpListener::bind(addr)?;
                let addr = listener.local_addr()?.to_string();
                Ok(Listener::Tcp(Addr::Tcp(addr), listener))
            }
            #[cfg(unix)]
            Addr::Unix(path) => {
                let listener = UnixListener::bind(path)?;
                let addr = path.clone();
                Ok(Listener::Unix(Addr::Unix(addr), listener))
            }
        })
    }
}

pub enum Listener {
    Tcp(Addr, TcpListener),
    Unix(Addr, UnixListener),
}

impl Listener {
    pub fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp(_, _))
    }

    pub fn is_unix(&self) -> bool {
        !self.is_tcp()
    }

    pub fn local_addr(&self) -> &Addr {
        match self {
            Listener::Tcp(a, _) => a,
            Listener::Unix(a, _) => a,
        }
    }

    pub fn accept(&self) -> io::Result<(ReadHalf, WriteHalf)> {
        explain_io(self.local_addr(), || match self {
            Listener::Tcp(_, lis) => {
                let (stream1, _) = lis.accept()?;
                let stream2 = stream1.try_clone()?;
                Ok((ReadHalf::Tcp(stream1), WriteHalf::Tcp(stream2)))
            }
            Listener::Unix(_, lis) => {
                let (stream1, _) = lis.accept()?;
                let stream2 = stream1.try_clone()?;
                Ok((ReadHalf::Unix(stream1), WriteHalf::Unix(stream2)))
            }
        })
    }
}

#[derive(Debug)]
pub enum ReadHalf {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl io::Read for ReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            ReadHalf::Tcp(conn) => conn.read(buf),
            ReadHalf::Unix(conn) => conn.read(buf),
        }
    }
}

impl Drop for ReadHalf {
    fn drop(&mut self) {
        let _ = match self {
            ReadHalf::Tcp(conn) => conn.shutdown(Shutdown::Read),
            ReadHalf::Unix(conn) => conn.shutdown(Shutdown::Read),
        };
    }
}

impl ReadHalf {
    pub fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp(_))
    }

    pub fn is_unix(&self) -> bool {
        !self.is_tcp()
    }
}

#[derive(Debug)]
pub enum WriteHalf {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl io::Write for WriteHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            WriteHalf::Tcp(conn) => conn.write(buf),
            WriteHalf::Unix(conn) => conn.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            WriteHalf::Tcp(conn) => conn.flush(),
            WriteHalf::Unix(conn) => conn.flush(),
        }
    }
}

impl Drop for WriteHalf {
    fn drop(&mut self) {
        let _ = match self {
            WriteHalf::Tcp(conn) => conn.shutdown(Shutdown::Write),
            WriteHalf::Unix(conn) => conn.shutdown(Shutdown::Write),
        };
    }
}

impl WriteHalf {
    pub fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp(_))
    }

    pub fn is_unix(&self) -> bool {
        !self.is_tcp()
    }
}
