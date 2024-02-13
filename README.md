MAPI PROXY
==========

Proxy to inspect [MonetDB] network traffic.
Pretty-prints raw bytes, low-level blocks or higher-level messages.

Usage:

```plain
Usage: mapiproxy [OPTIONS] LISTEN_ADDR FORWARD_ADDR

LISTEN_ADDR and FORWARD_ADDR:
    port, for example, 50000
    host:port, for example, localhost:50000 or 127.0.0.1:50000
    /path/to/unixsock, for example, /tmp/.s.monetdb.50000

Options:
    -m --messages       Dump whole messages
    -b --blocks         Dump individual blocks
    -r --raw            Dump bytes as they come in
    -B --binary         Force dumping as binary
    --help              This help
    --version           Version info
```

[MonetDB]: https://www.monetdb.org/