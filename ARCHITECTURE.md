Code layout overview
====================

In main.rs we take care of setting everything up.

In reactor.rs we do all socket i/o.
The reactor reports events, for example,

* new connection came in on ip:port from ip:port, succesfully
  forwarded to ip:port, or connection refused by host:port.

* proxy inserted/deleted '0' byte 

* bytes received from client

* bytes received from server

* client/server closed read/write end of the connection,
  N bytes could not be delivered.

* Out of Band (OOB) message detected, forwarded

* Connection fully closed

OOB handling is inherently lossy, the location in the TCP stream where it
occurred cannot be preserved.

In mapi.rs we take the events and reconstruct MAPI blocks and messages from
them, and format them. Formatting also involves replacing CR with '↵', TAB with
'→', etc.

Finally, dump.rs contains the code that wraps the formatted messages in
visual block boundaries and writes them to stdout. Possibly, in the future
dump.rs may offer an API that allows the MAPI module to use some colors, etc,
depending on the capabilities of the output device.

Module network.rs exists to paper over the differences between TCP sockets
and Unix Domain sockets.


To do
=====

1. Implement a TCP reactor using mio

   (why mio? tokio doesn't seem to expose OOB and using mio directly actually seems
   to simplify some things, it's kind of made for it)

2. Generalize it to Unix

   - socket types

   - '0' insertion/removal

3. Add / import MAPI parsing

