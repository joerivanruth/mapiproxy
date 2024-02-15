use std::panic::PanicInfo;
use std::process::ExitCode;
use std::{io, panic, process, thread};

use anyhow::Result as AResult;
use argsplitter::{ArgError, ArgSplitter};

use crate::{
    mapi,
    proxy::{event::EventSink, network::MonetAddr, Proxy},
    render::Renderer,
    Level, VERSION,
};

pub const USAGE: &str = "\
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
    --help              This help
    --version           Version info
";

pub fn main() -> ExitCode {
    argsplitter::main_support::report_errors(USAGE, mymain())
}

fn mymain() -> AResult<()> {
    install_panic_hook();

    let mut level = Level::Messages;
    let mut force_binary = false;

    let mut args = ArgSplitter::from_env();
    while let Some(flag) = args.flag()? {
        match flag {
            "-m" | "--messages" => level = Level::Messages,
            "-b" | "--blocks" => level = Level::Blocks,
            "-r" | "--raw" => level = Level::Raw,
            "-B" | "--binary" => force_binary = true,
            "--help" => {
                println!("Mapiproxy version {VERSION}");
                println!();
                println!("{USAGE}");
                return Ok(());
            }
            "--version" => {
                println!("Mapiproxy version {VERSION}");
                return Ok(());
            }
            _ => return Err(ArgError::unknown_flag(flag).into()),
        }
    }
    let listen_addr: MonetAddr = args.stashed_os("LISTEN ADDR")?.try_into()?;
    let forward_addr: MonetAddr = args.stashed_os("FORWARD_ADDR")?.try_into()?;
    args.no_more_stashed()?;

    let mut renderer = Renderer::new(io::stdout());

    let (send_events, receive_events) = std::sync::mpsc::sync_channel(500);
    let sink = EventSink::new(move |event| {
        let _ = send_events.send(event);
    });
    let mut proxy = Proxy::new(listen_addr, forward_addr, sink)?;
    thread::spawn(move || proxy.run().unwrap());

    let renderer: &mut Renderer = &mut renderer;
    let mut state = mapi::State::new(level, force_binary);
    loop {
        let ev = receive_events.recv()?;
        state.handle(&ev, renderer)?;
    }
}

fn install_panic_hook() {
    let orig_hook = panic::take_hook();
    let my_hook = Box::new(move |panic_info: &PanicInfo<'_>| {
        orig_hook(panic_info);
        process::exit(1);
    });
    panic::set_hook(my_hook);
}