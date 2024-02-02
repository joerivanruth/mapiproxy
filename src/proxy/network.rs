#![allow(dead_code)]

use std::{
    borrow::Cow,
    ffi::{OsStr, OsString},
    fmt::Display,
    io::{self, ErrorKind},
    net::{self, ToSocketAddrs},
    path::{Path, PathBuf},
};

use mio::net::{TcpListener, TcpStream, UnixListener, UnixStream};

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum MonetAddr {
    Tcp { host: String, port: u16 },
    Unix(PathBuf),
    PortOnly(u16),
}

#[derive(Debug, Clone)]
pub enum Addr {
    Tcp(net::SocketAddr),
    Unix(PathBuf),
}

#[derive(Debug)]
pub enum MioListener {
    Tcp(TcpListener),
    Unix(UnixListener),
}

#[derive(Debug)]
pub enum MioStream {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl Display for MonetAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MonetAddr::Tcp { host, port } => write!(f, "{host}:{port}"),
            MonetAddr::Unix(path) => path.display().fmt(f),
            MonetAddr::PortOnly(n) => n.fmt(f),
        }
    }
}

impl TryFrom<&OsStr> for MonetAddr {
    type Error = io::Error;

    fn try_from(os_value: &OsStr) -> Result<Self, Self::Error> {
        let str_value = os_value.to_string_lossy();

        let make_error = || {
            io::Error::new(
                ErrorKind::InvalidInput,
                format!("invalid address: {}", str_value),
            )
        };

        // If it contains slashes or backslashes, it must be a path
        let bytes = os_value.as_encoded_bytes();
        if bytes.contains(&b'/') || bytes.contains(&b'\\') {
            return Ok(MonetAddr::Unix(os_value.into()));
        }

        // The other possibilities are all proper str's
        let Cow::Borrowed(str_value) = str_value else {
            return Err(make_error());
        };

        // If it's a number, it must be the port number.
        if let Ok(port) = str_value.parse() {
            return Ok(MonetAddr::PortOnly(port));
        }

        // If it ends in :DIGITS, it must be a host:port pair
        if let Some(colon) = str_value.rfind(':') {
            if let Ok(port) = str_value[colon + 1..].parse() {
                let host = str_value[..colon].to_string();
                return Ok(MonetAddr::Tcp { host, port });
            }
        }

        Err(make_error())
    }
}

impl TryFrom<OsString> for MonetAddr {
    type Error = io::Error;

    fn try_from(value: OsString) -> Result<Self, Self::Error> {
        Self::try_from(value.as_os_str())
    }
}

impl MonetAddr {
    pub fn resolve(&self) -> io::Result<Vec<Addr>> {
        let mut addrs = self.resolve_unix()?;
        let tcp_addrs = self.resolve_tcp()?;
        addrs.extend(tcp_addrs);
        Ok(addrs)
    }

    pub fn resolve_tcp(&self) -> io::Result<Vec<Addr>> {
        let (host, port) = match self {
            MonetAddr::Unix(_) => return Ok(vec![]),
            MonetAddr::Tcp { host, port } => (host.as_str(), *port),
            MonetAddr::PortOnly(port) => ("localhost", *port),
        };
        let resolved = (host, port).to_socket_addrs()?;
        let addrs = resolved.into_iter().map(Addr::Tcp).collect();
        Ok(addrs)
    }

    pub fn resolve_unix(&self) -> io::Result<Vec<Addr>> {
        // todo!
        Ok(vec![])
    }
}

impl Display for Addr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Addr::Tcp(a) => a.fmt(f),
            Addr::Unix(path) => path.display().fmt(f),
        }
    }
}

impl Addr {
    pub fn listen(&self) -> io::Result<MioListener> {
        let listener = match self {
            Addr::Tcp(a) => MioListener::Tcp(TcpListener::bind(a.clone())?),
            Addr::Unix(a) => MioListener::Unix(UnixListener::bind(a.clone())?),
        };
        Ok(listener)
    }

    pub fn connect(&self) -> io::Result<MioStream> {
        let conn = match self {
            Addr::Tcp(a) => MioStream::Tcp(TcpStream::connect(a.clone())?),
            Addr::Unix(a) => MioStream::Unix(UnixStream::connect(a)?),
        };
        Ok(conn)
    }
}

impl From<net::SocketAddr> for Addr {
    fn from(value: net::SocketAddr) -> Self {
        Addr::Tcp(value)
    }
}

impl From<PathBuf> for Addr {
    fn from(value: PathBuf) -> Self {
        Addr::Unix(value)
    }
}

impl From<mio::net::SocketAddr> for Addr {
    fn from(value: mio::net::SocketAddr) -> Self {
        value.as_pathname().unwrap_or(Path::new("<UNNAMED>")).to_path_buf().into()
    }
}

impl mio::event::Source for MioListener {
    fn register(
        &mut self,
        registry: &mio::Registry,
        token: mio::Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        match self {
            Self::Tcp(lis) => lis.register(registry, token, interests),
            Self::Unix(lis) => lis.register(registry, token, interests),
        }
    }

    fn reregister(
        &mut self,
        registry: &mio::Registry,
        token: mio::Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        match self {
            Self::Tcp(lis) => lis.reregister(registry, token, interests),
            Self::Unix(lis) => lis.reregister(registry, token, interests),
        }
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        match self {
            Self::Tcp(lis) => lis.deregister(registry),
            Self::Unix(lis) => lis.deregister(registry),
        }
    }
}

impl MioListener {
    pub fn accept(&self) -> io::Result<(MioStream, Addr)> {
        match self {
            MioListener::Tcp(lis) => {
                let (conn, peer) = lis.accept()?;
                let stream = MioStream::Tcp(conn);
                let peer = Addr::Tcp(peer);
                Ok((stream, peer))
            }
            MioListener::Unix(lis) => {
                let (conn, peer) = lis.accept()?;
                let stream = MioStream::Unix(conn);
                Ok((stream, peer.into()))
            }
        }
    }
}

impl mio::event::Source for MioStream {
    fn register(
        &mut self,
        registry: &mio::Registry,
        token: mio::Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        match self {
            Self::Tcp(lis) => lis.register(registry, token, interests),
            Self::Unix(lis) => lis.register(registry, token, interests),
        }
    }

    fn reregister(
        &mut self,
        registry: &mio::Registry,
        token: mio::Token,
        interests: mio::Interest,
    ) -> io::Result<()> {
        match self {
            Self::Tcp(lis) => lis.reregister(registry, token, interests),
            Self::Unix(lis) => lis.reregister(registry, token, interests),
        }
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        match self {
            Self::Tcp(lis) => lis.deregister(registry),
            Self::Unix(lis) => lis.deregister(registry),
        }
    }
}

impl MioStream {
    pub fn take_error(&self) -> Result<Option<io::Error>, io::Error> {
        match self {
            MioStream::Tcp(s) => s.take_error(),
            MioStream::Unix(s) => s.take_error(),
        }
    }

    pub fn peer_addr(&self) -> Result<Addr, io::Error> {
        let addr = match self {
            MioStream::Tcp(s) => s.peer_addr()?.into(),
            MioStream::Unix(s) => s.peer_addr()?.into(),
        };
        Ok(addr)
    }

    pub fn shutdown(&self, shutdown: net::Shutdown) -> io::Result<()> {
        match self {
            MioStream::Tcp(s) => s.shutdown(shutdown),
            MioStream::Unix(s) => s.shutdown(shutdown),
        }
    }
}

impl io::Write for MioStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            MioStream::Tcp(s) => s.write(buf),
            MioStream::Unix(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            MioStream::Tcp(s) => s.flush(),
            MioStream::Unix(s) => s.flush(),
        }    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        match self {
            MioStream::Tcp(s) => s.write_vectored(bufs),
            MioStream::Unix(s) => s.write_vectored(bufs),
        }
    }
}

impl io::Read for MioStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            MioStream::Tcp(s) => s.read(buf),
            MioStream::Unix(s) => s.read(buf),
        }
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        match self {
            MioStream::Tcp(s) => s.read_vectored(bufs),
            MioStream::Unix(s) => s.read_vectored(bufs),
        }
    }
}