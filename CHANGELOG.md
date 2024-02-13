# Change Log

What changed in mapiproxy, per version


## mapiproxy NEXTVERSION - YYYY-MM-DD

The basics work:

- Listen on TCP sockets and Unix Domain sockets

- Connect to TCP sockets and Unix Domain sockets

- Adjust the initial '0' (0x30) byte when proxying between Unix and TCP or vice
  versa

- Render as raw reads and writes, full MAPI blocks and full MAPI messages

- Render as text or as a hex dump

- Pretty-print tabs and newlines

- In raw mode, highlight the MAPI block headers


## mapiproxy 0.3.0

Skipped due to experiments in another repo.


## mapiproxy 0.2.0

Skipped because predecessor 'monetproxy' was already at 0.2.0.


## mapiproxy 0.1.0 - unreleased

Initial version number picked by 'cargo new'
