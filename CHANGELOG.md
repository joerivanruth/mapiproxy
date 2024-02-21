# Change Log

What changed in mapiproxy, per version


## mapiproxy NEXTVERSION - YYYY-MM-DD

- Add option --color=always|never|auto to control the use of color escapes.
  'Auto' is 'on' on terminals, 'off' otherwise.

- Colorize text, digits and whitespace in binary output. This makes it easier
  to match the hex codes on the left to the characters on the right.

- Support raw IPv6 addresses in LISTEN_ADDR and FORWARD_ADDR, between square brackets.
  For example, `[::1]:50000`.

- Clean up Unix sockets when Control-C is pressed.


## mapiproxy 0.5.1 - 2024-02-16

- This release only exists because a version v0.5.1-alpha.1
  was uploaded to crates.io as an experiment.


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
