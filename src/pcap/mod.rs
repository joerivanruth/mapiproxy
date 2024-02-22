
use std::io;

use anyhow::Result as AResult;

use crate::mapi;

pub fn parse_pcap_file(_rd: impl io::Read + 'static, _mapi_state: mapi::State) -> AResult<()> {
    todo!()
}
