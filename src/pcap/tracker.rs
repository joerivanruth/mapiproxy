use std::{io, net::IpAddr};

use anyhow::{bail, Result as AResult};
use etherparse::{InternetSlice, Ipv4Slice, Ipv6Slice, SlicedPacket, TcpSlice, TransportSlice};

use crate::proxy::event::MapiEvent;

use super::tcp::TcpTracker;

/// Struct Tracker holds the state necessary to process packets and emit MapiEvents.
pub struct Tracker<'a> {
    handler: Box<dyn FnMut(MapiEvent) -> io::Result<()> + 'a>,
    tcp_tracker: TcpTracker,
}

impl<'a> Tracker<'a> {
    /// Create a new Tracker which calls the given closure for each MapiEvent it needs to emit.
    pub fn new(event_handler: impl FnMut(MapiEvent) -> io::Result<()> + 'a) -> Self {
        let handler = Box::new(event_handler);
        Tracker {
            handler,
            tcp_tracker: TcpTracker::new(),
        }
    }

    /// Process the given packet as an Ethernet frame.
    pub fn process_ethernet(&mut self, data: &[u8]) -> AResult<()> {
        let ether_slice = SlicedPacket::from_ethernet(data)?;
        let transport_slice = ether_slice.transport.as_ref();
        match &ether_slice.net {
            Some(InternetSlice::Ipv4(inet4)) => self.handle_ipv4(inet4, transport_slice),
            Some(InternetSlice::Ipv6(inet6)) => self.handle_ipv6(inet6, transport_slice),
            None => Ok(()),
        }
    }

    /// Examine IPv6 packet. If it's a TCP packet and not fragmented, hand it to [Self::handle_tcp]
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

    /// Examine IPv4 packet. If it's a TCP packet and not fragmented, hand it to [Self::handle_tcp]
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

    /// Called by [Self::handle_ipv4] and [Self::handle_ipv6] when they encounter TCP traffic
    pub fn handle_tcp(&mut self, src: IpAddr, dest: IpAddr, tcp: &TcpSlice) -> AResult<()> {
        // It's nice for handle_ipv4 and handle_ipv6 to simply call handle_tcp, but it turns
        // out that the actual handling is done by the [TcpTracker] subobject.
        self.tcp_tracker.handle(src, dest, tcp, &mut self.handler)?;
        Ok(())
    }
}
