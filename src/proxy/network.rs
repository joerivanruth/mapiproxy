use std::{
    ffi::{OsStr, OsString},
    fmt::Display,
    io::{self, ErrorKind},
    net::{self, IpAddr, SocketAddr as TcpSocketAddr, ToSocketAddrs},
    path::PathBuf,
};

// These are only used by Unix Domain socket code
#[cfg(unix)]
use std::{fs, path::Path};

use lazy_regex::{regex_captures, regex_is_match};
#[cfg(unix)]
use mio::net::{SocketAddr as UnixSocketAddr, UnixListener, UnixStream};
use mio::net::{TcpListener, TcpStream};

#[cfg(not(unix))]
fn unix_not_supported() -> io::Error {
    io::Error::new(
        ErrorKind::Unsupported,
        "Unix Domain sockets are not supported on this system",
    )
}

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum MonetAddr {
    Dns { host: String, port: u16 },
    Ip { ip: IpAddr, port: u16 },
    Unix(PathBuf),
    PortOnly(u16),
}

#[derive(Debug, Clone)]
pub enum Addr {
    Tcp(TcpSocketAddr),
    Unix(PathBuf),
}

#[derive(Debug)]
pub enum MioListener {
    Tcp(TcpListener),
    #[cfg(unix)]
    Unix(UnixListener),
}

#[derive(Debug)]
pub enum MioStream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

impl Display for MonetAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // MonetAddr::Tcp { host, port } => write!(f, "{host}:{port}"),
            MonetAddr::Dns { host, port } => write!(f, "{host}:{port}"),
            MonetAddr::Ip {
                ip: IpAddr::V4(ip4),
                port,
            } => write!(f, "{ip4}:{port}"),
            MonetAddr::Ip {
                ip: IpAddr::V6(ip6),
                port,
            } => write!(f, "[{ip6}]:{port}"),
            MonetAddr::Unix(path) => path.display().fmt(f),
            MonetAddr::PortOnly(n) => n.fmt(f),
        }
    }
}

impl TryFrom<&OsStr> for MonetAddr {
    type Error = io::Error;

    fn try_from(os_value: &OsStr) -> Result<Self, io::Error> {
        // this function does all the work but it returns Option rather
        // than Result.
        fn parse(os_value: &OsStr) -> Option<MonetAddr> {
            // If it contains slashes or backslashes, it must be a path
            let bytes = os_value.as_encoded_bytes();
            if bytes.contains(&b'/') || bytes.contains(&b'\\') {
                return Some(MonetAddr::Unix(os_value.into()));
            }

            // The other possibilities are all proper str's
            let str_value = os_value.to_str()?;

            // If it's a number, it must be the port number.
            if let Ok(port) = str_value.parse() {
                return Some(MonetAddr::PortOnly(port));
            }

            // it must end in :PORTNUMBER
            let (_, host_part, port_part) = regex_captures!(r"^(.+):(\d+)$", str_value)?;
            let port: u16 = port_part.parse().ok()?;

            // is the host IPv4, IPv6 or DNS?
            if regex_is_match!(r"^\d+.\d+.\d+.\d+$", host_part) {
                // IPv4
                Some(MonetAddr::Ip {
                    ip: IpAddr::V4(host_part.parse().ok()?),
                    port,
                })
            } else if let Some((_, ip)) = regex_captures!(r"^\[([0-9a-f:]+)\]$"i, host_part) {
                // IPv6
                Some(MonetAddr::Ip {
                    ip: IpAddr::V6(ip.parse().ok()?),
                    port,
                })
            } else if regex_is_match!(r"^[a-z0-9][-a-z0-9.]*$"i, host_part) {
                // names consisting of letters, digits and hyphens, separated or terminated by periods
                Some(MonetAddr::Dns {
                    host: host_part.to_string(),
                    port,
                })
            } else {
                None
            }
        }

        if let Some(monetaddr) = parse(os_value) {
            Ok(monetaddr)
        } else {
            Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("invalid address: {}", os_value.to_string_lossy()),
            ))
        }
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
        fn gather<T: ToSocketAddrs>(a: T) -> io::Result<Vec<Addr>> {
            Ok(a.to_socket_addrs()?.map(Addr::Tcp).collect())
        }

        match self {
            MonetAddr::Unix(_) => Ok(vec![]),
            MonetAddr::Dns { host, port } => gather((host.as_str(), *port)),
            MonetAddr::Ip { ip, port } => gather((*ip, *port)),
            MonetAddr::PortOnly(port) => gather(("localhost", *port)),
        }
    }

    pub fn resolve_unix(&self) -> io::Result<Vec<Addr>> {
        if cfg!(unix) {
            let path = match self {
                MonetAddr::Dns { .. } | MonetAddr::Ip { .. } => return Ok(vec![]),
                MonetAddr::Unix(p) => p.clone(),
                MonetAddr::PortOnly(port) => PathBuf::from(format!("/tmp/.s.monetdb.{port}")),
            };
            Ok(vec![Addr::Unix(path)])
        } else {
            Ok(vec![])
        }
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
    pub fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp(_))
    }

    pub fn is_unix(&self) -> bool {
        !self.is_tcp()
    }

    pub fn listen(&self) -> io::Result<MioListener> {
        let listener = match self {
            Addr::Tcp(a) => MioListener::Tcp(TcpListener::bind(*a)?),
            #[cfg(unix)]
            Addr::Unix(a) => {
                let listener = match UnixListener::bind(a) {
                    Ok(lis) => lis,
                    Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                        fs::remove_file(a)?;
                        UnixListener::bind(a)?
                    }
                    Err(other) => return Err(other),
                };
                MioListener::Unix(listener)
            }
            #[cfg(not(unix))]
            Addr::Unix(_) => return Err(unix_not_supported()),
        };
        Ok(listener)
    }

    pub fn connect(&self) -> io::Result<MioStream> {
        let conn = match self {
            Addr::Tcp(a) => MioStream::Tcp(TcpStream::connect(*a)?),
            #[cfg(unix)]
            Addr::Unix(a) => MioStream::Unix(UnixStream::connect(a)?),
            #[cfg(not(unix))]
            Addr::Unix(_) => return Err(unix_not_supported()),
        };
        Ok(conn)
    }
}

impl From<TcpSocketAddr> for Addr {
    fn from(value: TcpSocketAddr) -> Self {
        Addr::Tcp(value)
    }
}

impl From<PathBuf> for Addr {
    fn from(value: PathBuf) -> Self {
        Addr::Unix(value)
    }
}

#[cfg(unix)]
impl From<UnixSocketAddr> for Addr {
    fn from(value: UnixSocketAddr) -> Self {
        value
            .as_pathname()
            .unwrap_or(Path::new("<UNNAMED>"))
            .to_path_buf()
            .into()
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
            #[cfg(unix)]
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
            #[cfg(unix)]
            Self::Unix(lis) => lis.reregister(registry, token, interests),
        }
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        match self {
            Self::Tcp(lis) => lis.deregister(registry),
            #[cfg(unix)]
            Self::Unix(lis) => lis.deregister(registry),
        }
    }
}

impl MioListener {
    #[allow(dead_code)]
    pub fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp(_))
    }

    #[allow(dead_code)]
    pub fn is_unix(&self) -> bool {
        !self.is_tcp()
    }

    pub fn accept(&self) -> io::Result<(MioStream, Addr)> {
        match self {
            MioListener::Tcp(lis) => {
                let (conn, peer) = lis.accept()?;
                let stream = MioStream::Tcp(conn);
                let peer = Addr::Tcp(peer);
                Ok((stream, peer))
            }
            #[cfg(unix)]
            MioListener::Unix(lis) => {
                let (conn, peer) = lis.accept()?;
                let stream = MioStream::Unix(conn);
                Ok((stream, peer.into()))
            }
        }
    }
}

impl Drop for MioListener {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let MioListener::Unix(listener) = self {
            let Ok(unix_sock_addr) = listener.local_addr() else {
                return;
            };
            let Some(path) = unix_sock_addr.as_pathname() else {
                return;
            };
            let _ = fs::remove_file(path);
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
            #[cfg(unix)]
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
            #[cfg(unix)]
            Self::Unix(lis) => lis.reregister(registry, token, interests),
        }
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        match self {
            Self::Tcp(lis) => lis.deregister(registry),
            #[cfg(unix)]
            Self::Unix(lis) => lis.deregister(registry),
        }
    }
}

impl MioStream {
    pub fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp(_))
    }

    pub fn is_unix(&self) -> bool {
        !self.is_tcp()
    }

    pub fn established(&self) -> io::Result<Option<Addr>> {
        if let Err(e) | Ok(Some(e)) = self.take_error() {
            return Err(e);
        }

        let peer_result = match self {
            MioStream::Tcp(s) => s.peer_addr().map(Addr::from),
            #[cfg(unix)]
            MioStream::Unix(s) => s.peer_addr().map(Addr::from),
        };

        match peer_result {
            Ok(addr) => Ok(Some(addr)),
            Err(e) => match e.kind() {
                ErrorKind::WouldBlock | ErrorKind::NotConnected => Ok(None),
                _ => Err(e),
            },
        }
    }

    pub fn shutdown(&self, shutdown: net::Shutdown) -> io::Result<()> {
        match self {
            MioStream::Tcp(s) => s.shutdown(shutdown),
            #[cfg(unix)]
            MioStream::Unix(s) => s.shutdown(shutdown),
        }
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        match self {
            MioStream::Tcp(s) => s.take_error(),
            #[cfg(unix)]
            MioStream::Unix(s) => s.take_error(),
        }
    }

    #[allow(dead_code)]
    pub fn peer_addr(&self) -> io::Result<Addr> {
        let addr = match self {
            MioStream::Tcp(s) => s.peer_addr()?.into(),
            #[cfg(unix)]
            MioStream::Unix(s) => s.peer_addr()?.into(),
        };
        Ok(addr)
    }

    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        match self {
            MioStream::Tcp(s) => s.set_nodelay(nodelay),
            #[cfg(unix)]
            MioStream::Unix(_) => Ok(()),
        }
    }
}

impl io::Write for MioStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            MioStream::Tcp(s) => s.write(buf),
            #[cfg(unix)]
            MioStream::Unix(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            MioStream::Tcp(s) => s.flush(),
            #[cfg(unix)]
            MioStream::Unix(s) => s.flush(),
        }
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        match self {
            MioStream::Tcp(s) => s.write_vectored(bufs),
            #[cfg(unix)]
            MioStream::Unix(s) => s.write_vectored(bufs),
        }
    }
}

impl io::Read for MioStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            MioStream::Tcp(s) => s.read(buf),
            #[cfg(unix)]
            MioStream::Unix(s) => s.read(buf),
        }
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        match self {
            MioStream::Tcp(s) => s.read_vectored(bufs),
            #[cfg(unix)]
            MioStream::Unix(s) => s.read_vectored(bufs),
        }
    }
}
