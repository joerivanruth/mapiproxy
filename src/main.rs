mod mapi;
mod proxy;
mod render;

use std::panic::PanicInfo;
use std::process::ExitCode;
use std::{io, panic, process, thread};

use anyhow::Result as AResult;
use argsplitter::{ArgError, ArgSplitter};

use proxy::event::EventSink;
use proxy::network::Addr;
use proxy::Proxy;
use render::Renderer;

use crate::proxy::event::MapiEvent;

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
    install_panic_hook();

    let mut level = Level::Blocks;
    let mut force_binary = false;

    let mut args = ArgSplitter::from_env();
    while let Some(flag) = args.flag()? {
        match flag {
            "-m" | "--messages" => level = Level::Messages,
            "-b" | "--blocks" => level = Level::Blocks,
            "-r" | "--raw" => level = Level::Raw,
            "-B" | "--binary" => force_binary = true,
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

    let mut renderer = Renderer::new(io::stdout());

    let (send_events, receive_events) = std::sync::mpsc::sync_channel(20);
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

#[allow(dead_code)]
fn print_nondata_event(renderer: &mut Renderer, ev: &MapiEvent) -> AResult<()> {
    use MapiEvent::*;
    match ev {
        BoundPort(port) => 
            renderer.message(None, None, format_args!("LISTEN on port {port}"))?,
        Incoming { id, local, peer } => renderer.message(
            Some(*id),
            None,
            format_args!("INCOMING on {local} from {peer}"),
        )?,
        Connecting { id, remote } => {
            renderer.message(Some(*id), None, format_args!("CONNECTING to {remote}"))?
        }
        Connected { id, .. } => renderer.message(Some(*id), None, "CONNECTED")?,
        End { id } => renderer.message(Some(*id), None, "ENDED")?,
        Aborted { id, error } => {
            renderer.message(Some(*id), None, format_args!("ABORTED: {error}"))?
        }
        Data { .. } => {}
        ShutdownRead { id, direction } => {
            renderer.message(Some(*id), Some(*direction), "shut down reading")?
        }
        ShutdownWrite {
            id,
            direction,
            discard: 0,
        } => renderer.message(Some(*id), Some(*direction), "shut down writing")?,
        ShutdownWrite {
            id,
            direction,
            discard: n,
        } => renderer.message(
            Some(*id),
            Some(*direction),
            format_args!("Shut down writing, discarding {n} unsent bytes"),
        )?,
    }

    Ok(())
}
