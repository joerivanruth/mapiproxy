mod mybufread;
mod tracker;

use std::io;

use anyhow::{bail, Result as AResult};

use pcap_file::pcap::PcapReader;

use crate::mapi;

use self::{mybufread::MyBufReader, tracker::Tracker};

pub fn parse_pcap_file(mut rd: impl io::Read + 'static, mapi_state: mapi::State) -> AResult<()> {
    // read ahead to inspect the file header
    let mut signature = [0u8; 4];
    rd.read_exact(&mut signature)?;

    // create a MyBufReader, which is basically a BufReader except
    // that we preload it with the bytes we read above
    let mut buffer = Vec::with_capacity(16384);
    buffer.extend_from_slice(&signature);
    let mybufreader = MyBufReader::new(rd, buffer);

    let tracker = Tracker::new(mapi_state);

    // Pass the file to either the legacy pcap reader or the pcapng reader
    match signature {
        [0xD4, 0xC3, 0xB2, 0xA1] | [0xA1, 0xB2, 0xB3, 0xD4] => {
            parse_legacy_pcap(mybufreader, tracker)
        }
        _ => bail!(
            "Unknown pcap file signature {:02X} {:02X} {:02X} {:02X}",
            signature[0],
            signature[1],
            signature[2],
            signature[3]
        ),
    }
}

fn parse_legacy_pcap(rd: MyBufReader, mut tracker: Tracker) -> AResult<()> {
    let mut pcap_reader = PcapReader::new(rd).unwrap();

    let header = pcap_reader.header();
    eprintln!("{header:?}");

    while let Some(pkt) = pcap_reader.next_packet() {
        let pkt = pkt?;
        if pkt.data.len() == header.snaplen as usize {
            bail!("truncated packet");
        }

        tracker.process_packet(header.datalink, &pkt.data)?;
    }

    Ok(())
}
