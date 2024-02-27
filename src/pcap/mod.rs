mod mybufread;
mod tcp;
mod tracker;

use std::io;

use anyhow::{bail, Result as AResult};

use pcap_file::{
    pcap::PcapReader,
    pcapng::{Block, PcapNgReader},
    DataLink,
};

use self::mybufread::MyBufReader;
pub use self::tracker::Tracker;

/// Parse PCAP records from the reader and hand the packets to the Tracker. This
/// function works with both the old-style PCAP and with PCAP-NG file formats.
pub fn parse_pcap_file(mut rd: impl io::Read, tracker: &mut Tracker) -> AResult<()> {
    // read ahead to inspect the file header
    let mut signature = [0u8; 4];
    rd.read_exact(&mut signature)?;

    // create a MyBufReader, which is basically a BufReader except
    // that we preload it with the bytes we read above
    let mut buffer = Vec::with_capacity(16384);
    buffer.extend_from_slice(&signature);
    let mybufreader = MyBufReader::new(rd, buffer);

    // Pass the file to either the legacy pcap reader or the pcapng reader
    match signature {
        [0xD4, 0xC3, 0xB2, 0xA1] | [0xA1, 0xB2, 0xB3, 0xD4] => {
            parse_legacy_pcap(mybufreader, tracker)
        }
        [0x0A, 0x0D, 0x0D, 0x0A] => parse_pcap_ng(mybufreader, tracker),
        _ => bail!(
            "Unknown pcap file signature {:02X} {:02X} {:02X} {:02X}",
            signature[0],
            signature[1],
            signature[2],
            signature[3]
        ),
    }
}

/// Parse the file as legacy PCAP and pass the packets to [process_packet]
fn parse_legacy_pcap(rd: MyBufReader, tracker: &mut Tracker) -> AResult<()> {
    let mut pcap_reader = PcapReader::new(rd)?;

    let header = pcap_reader.header();

    while let Some(pkt) = pcap_reader.next_packet() {
        let pkt = pkt?;
        if pkt.data.len() == header.snaplen as usize {
            bail!("truncated packet");
        }

        process_packet(header.datalink, &pkt.data, tracker)?;
    }

    Ok(())
}

/// Parse the file as PCAP-NG and pass the packets to [process_packet]
fn parse_pcap_ng(rd: MyBufReader, tracker: &mut Tracker) -> AResult<()> {
    let mut pcapng_reader = PcapNgReader::new(rd)?;

    // With PCAP-NG the linktype is not a file-global setting but it is set and
    // can theoretically be changed mid-file using Interface Description blocks.
    // This mutable holds the latest value we have seen.
    let mut linktype = None;

    while let Some(block) = pcapng_reader.next_block() {
        let data = match block? {
            Block::InterfaceDescription(iface) => {
                linktype = Some(iface.linktype);
                continue;
            }
            Block::Packet(packet) => packet.data,
            Block::SimplePacket(packet) => packet.data,
            Block::EnhancedPacket(packet) => packet.data,
            _ => continue,
        };

        // Broken files might contain packets before the first interface description block.
        // Ignore them.
        if let Some(lt) = linktype {
            process_packet(lt, &data, tracker)?;
        }
    }

    Ok(())
}

/// This function is called from both [parse_legacy_pcap] and [parse_pcap_ng]
/// for each packet in the file.
fn process_packet(linktype: DataLink, data: &[u8], tracker: &mut Tracker) -> AResult<()> {
    // We expect to read ethernet frames but it's also possible for pcap files to
    // capture at the IP level. Right now we only support Ethernet.
    match linktype {
        DataLink::ETHERNET => tracker.process_ethernet(data),
        _ => bail!("pcap file contains packet of type {linktype:?}, this is not supported"),
    }
}
