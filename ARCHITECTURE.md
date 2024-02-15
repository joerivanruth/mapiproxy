Code layout overview
====================

The proxy is written in Rust.

In main.rs we take care of setting everything up.

The `proxy` module takes care of all network IO. It's currently based on [mio],
a nonblocking IO library, because that seemed to be the best way to support
out-of-band messages. That requirement has gone away and mio could now probably
be replaced with a simpler threaded implementation, but that hasn't happened
yet.

The proxy is not aware of the structure of the communication protocol, it just
forwards bytes. There is one exception, when connecting to a Unix Domain socket,
the MAPI protocol requires the client to send an initial '0' (0x30) byte, the
proxy inserts this when forwarding a TCP connection to a Unix socket, and strips
it when forwarding a Unix connection to a TCP socket.

The proxy records what's going on by sending a series of `MapiEvent`s on a
channel. The main thread receives these messages and passes them to the `mapi`
module, which splits them into separate streams, one for each connection, and
analyzes the MAPI message structure. Depending on the mode it will display the
data immediately or it will buffer it to display complete MAPI blocks or
messages. The `mapi` module is also the place where it is decided whether to
display the data as a hex dump or as text.

The `mapi` module does not write output itself, instead it invokes methods on a
`Renderer` object passed by the main thread. The renderer can display
single-line messages or multi-line blocks with a header and a footer. The data
in the blocks can also be colored.

Currently there is only one `Renderer` implementation which renders the output
using Unicode drawing characters and vt100/ansi color escape codes. We plan to
soon add a mode where it omits the color escapes, and when writing protocol
documentation it might be useful to have a mode where the output is rendered in
HTML.


[mio]: https://github.com/tokio-rs/mio

