MAPI PROXY
==========

Proxy to inspect [MonetDB] network traffic.

[MonetDB]: https://www.monetdb.org/

The MonetDB network protocol is called MAPI, hence the name.

The proxy can listen on and connect to TCP sockets, Unix Domain sockets or both.
The MAPI protocol is slightly different between the two, the proxy will
automatically adjust it when proxying between the two.

Usage:

```plain
Usage: mapiproxy [OPTIONS] LISTEN_ADDR FORWARD_ADDR

LISTEN_ADDR and FORWARD_ADDR:
    port, for example, 50000
    host:port, for example, localhost:50000 or 127.0.0.1:50000
    /path/to/unixsock, for example, /tmp/.s.monetdb.50000

Options:
    -m --messages       Dump whole messages (default)
    -b --blocks         Dump individual blocks
    -r --raw            Dump bytes as they come in
    -B --binary         Force dumping as binary
    --color=WHEN        Colorize output, one of 'always', 'auto' or 'never'
    --help              This help
    --version           Version info
```
