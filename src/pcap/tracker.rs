use std::net::IpAddr;

use etherparse::{InternetSlice, Ipv4Slice, Ipv6Slice, SlicedPacket, TcpSlice, TransportSlice};
use anyhow::{bail, Result as AResult};

use crate::{mapi, render::Renderer};

pub struct Tracker<'a> {
    _state: mapi::State,
    renderer: &'a mut Renderer,
}

impl<'a> Tracker<'a> {
    pub fn new(state: mapi::State, renderer: &'a mut Renderer) -> Self {
        Tracker { _state: state, renderer }
    }

    pub fn process_ethernet(&mut self, data: &[u8]) -> AResult<()> {
        let ether_slice = SlicedPacket::from_ethernet(data)?;
        let transport_slice = ether_slice.transport.as_ref();
        match &ether_slice.net {
            Some(InternetSlice::Ipv4(inet4)) => self.parse_ipv4(inet4, transport_slice),
            Some(InternetSlice::Ipv6(inet6)) => self.parse_ipv6(inet6, transport_slice),
            None => Ok(())
        }
    }

    pub fn parse_ipv6(&mut self, ipv6: &Ipv6Slice, transport: Option<&TransportSlice>) -> AResult<()> {
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
        self.parse_tcp(src, dest, tcp)
    }

    pub fn parse_ipv4(&mut self, ipv4: &Ipv4Slice, transport: Option<&TransportSlice>) -> AResult<()> {
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
        self.parse_tcp(src, dest, tcp)
    }

    pub fn parse_tcp(&mut self, src: IpAddr, dest: IpAddr, tcp: &TcpSlice) -> AResult<()> {
        self.renderer.message(None, None, format_args!(
            "TCP {src} -> {dest}: {tcp:?}"
        ))?;
        Ok(())
    }
}
