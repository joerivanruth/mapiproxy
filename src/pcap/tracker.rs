use std::{io, net::IpAddr};

use anyhow::{bail, Result as AResult};
use etherparse::{InternetSlice, Ipv4Slice, Ipv6Slice, SlicedPacket, TcpSlice, TransportSlice};

use crate::proxy::event::MapiEvent;

use super::tcp::TcpTracker;

pub struct Tracker<'a> {
    handler: Box<dyn FnMut(MapiEvent) -> io::Result<()> + 'a>,
    tcp_tracker: TcpTracker,
}

impl<'a> Tracker<'a> {
    pub fn new(event_handler: impl FnMut(MapiEvent) -> io::Result<()> + 'a) -> Self {
        let handler = Box::new(event_handler);
        Tracker {
            handler,
            tcp_tracker: TcpTracker::new(),
        }
    }

    pub fn process_ethernet(&mut self, data: &[u8]) -> AResult<()> {
        let ether_slice = SlicedPacket::from_ethernet(data)?;
        let transport_slice = ether_slice.transport.as_ref();
        match &ether_slice.net {
            Some(InternetSlice::Ipv4(inet4)) => self.handle_ipv4(inet4, transport_slice),
            Some(InternetSlice::Ipv6(inet6)) => self.handle_ipv6(inet6, transport_slice),
            None => Ok(()),
        }
    }

    pub fn handle_ipv6(
        &mut self,
        ipv6: &Ipv6Slice,
        transport: Option<&TransportSlice>,
    ) -> AResult<()> {
        if ipv6.is_payload_fragmented() {
            bail!("pcap file contains fragmented ipv6 packet, not supported");
        }

        let tcp = match transport {
            None => bail!("transport not found, expected this only with fragmented packets"),
            Some(TransportSlice::Tcp(tcp)) => tcp,
            _ => return Ok(()),
        };

        let header = &ipv6.header();
        let src = IpAddr::from(header.source_addr());
        let dest = IpAddr::from(header.destination_addr());
        self.handle_tcp(src, dest, tcp)
    }

    pub fn handle_ipv4(
        &mut self,
        ipv4: &Ipv4Slice,
        transport: Option<&TransportSlice>,
    ) -> AResult<()> {
        if ipv4.is_payload_fragmented() {
            bail!("pcap file contains fragmented ipv4 packet, not supported");
        }

        let tcp = match transport {
            None => bail!("transport not found, expected this only with fragmented packets"),
            Some(TransportSlice::Tcp(tcp)) => tcp,
            _ => return Ok(()),
        };

        let header = &ipv4.header();
        let src = IpAddr::from(header.source_addr());
        let dest = IpAddr::from(header.destination_addr());
        self.handle_tcp(src, dest, tcp)
    }

    pub fn handle_tcp(&mut self, src: IpAddr, dest: IpAddr, tcp: &TcpSlice) -> AResult<()> {
        self.tcp_tracker.handle(src, dest, tcp, &mut self.handler)?;
        Ok(())
    }
}
