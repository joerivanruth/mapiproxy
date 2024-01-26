mod proxy;

use std::process::ExitCode;

use anyhow::Result as AResult;
use argsplitter::{ArgError, ArgSplitter};

use proxy::network::Addr;
use proxy::{EventSink, Proxy};

const USAGE: &str = "\
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
    --help              This help
";

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum Level {
    Raw,
    Blocks,
    Messages,
}

fn main() -> ExitCode {
    argsplitter::main_support::report_errors(USAGE, mymain())
}

fn mymain() -> AResult<()> {
    let mut _level = Level::Messages;
    let mut _force_binary = false;

    let mut args = ArgSplitter::from_env();
    while let Some(flag) = args.flag()? {
        match flag {
            "-m" | "--messages" => _level = Level::Messages,
            "-b" | "--blocks" => _level = Level::Blocks,
            "-r" | "--raw" => _level = Level::Raw,
            "-B" | "--binary" => _force_binary = true,
            "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => return Err(ArgError::unknown_flag(flag).into()),
        }
    }
    let listen_addr: Addr = args.stashed_os("LISTEN ADDR")?.try_into()?;
    let forward_addr: Addr = args.stashed_os("FORWARD_ADDR")?.try_into()?;
    args.no_more_stashed()?;

    let sink = EventSink::new(|event| println!("{event:?}"));
    let mut proxy = Proxy::new(listen_addr, forward_addr, sink)?;

    proxy.run()?;

    Ok(())
}
