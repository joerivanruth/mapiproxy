MAPI PROXY
==========

Proxy to inspect MonetDB/MAPI traffic. Pretty-prints raw bytes, low-level blocks
or higher-level messages.

Usage:

```plain
Usage: mapiproxy [OPTIONS] LISTEN_ADDR FORWARD_ADDR
Addr:
    PORT, for example 50000
    HOST:PORT, for example localhost:50000
    /PATH/TO/SOCK, for example, /tmp/.s.monetdb.50000
Options:
    -m --messages       Dump whole messages
    -b --blocks         Dump individual blocks
    -r --raw            Dump bytes as they come in
    -B --binary         Force dumping as binary
```

Mapiproxy dumps bytes when it receives them, before they have been forwarded
to the client. Sometimes this matters, keep it in mind.
Mapiproxy will report it if client or server closed the connection while
there were still pending bytes in Mapiproxy's buffers.

If the client or server close the connection when there is still data in
Mapiproxy's buffers, the number of bytes discarded is reported.
