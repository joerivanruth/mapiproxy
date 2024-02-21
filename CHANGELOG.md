# Change Log

What changed in mapiproxy, per version


## mapiproxy NEXTVERSION - YYYY-MM-DD

- The use of colors can now be configured with --color=always|never|auto.

- Raw IPv6 addresses are now allowed in LISTEN_ADDR and FORWARD_ADDR: `[::1]:50000`.

- Clean up Unix sockets when Control-C is pressed.


## mapiproxy 0.5.1 - 2024-02-16

- no user visible changes, release only because v0.5.1-alpha.1
  exists on crates.io.


## mapiproxy 0.5.0 - 2024-02-16

The basics work:

- Listen on TCP sockets and Unix Domain sockets

- Connect to TCP sockets and Unix Domain sockets

- Adjust the initial '0' (0x30) byte when proxying between Unix and TCP or vice
  versa

- Render either as raw reads and writes, full MAPI blocks or full MAPI messages

- Render as text or as a hex dump

- Pretty-print tabs and newlines

- In raw mode, highlight the MAPI block headers


## mapiproxy 0.3.0

Skipped due to experiments in another repo.


## mapiproxy 0.2.0

Skipped because predecessor 'monetproxy' was already at 0.2.0.


## mapiproxy 0.1.0 - unreleased

Initial version number picked by 'cargo new'
