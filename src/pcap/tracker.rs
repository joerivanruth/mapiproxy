use anyhow::Result as AResult;

use crate::mapi;

pub struct Tracker(mapi::State);

impl Tracker {
    pub fn new(mapi_state: mapi::State) -> Self {
        Tracker(mapi_state)
    }

    pub fn process_packet(&mut self, _datalink: pcap_file::DataLink, _data: &[u8]) -> AResult<()> {
        todo!()
    }
}
