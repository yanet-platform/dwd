use std::ffi::CString;

use crate::{error::Error, ffi};

/// Returns the number of ports (Ethernet devices) which are usable for the
/// application.
#[inline]
pub fn eth_dev_count() -> u16 {
    // SAFETY: FFI without arguments.
    unsafe { ffi::rte_eth_dev_count_avail() }
}

#[inline]
pub fn eth_dev_iter() -> EthDevIterator {
    EthDevIterator::new()
}

#[derive(Debug)]
pub struct EthDevIterator {
    port_id: Option<u16>,
    owner_id: u64,
}

impl EthDevIterator {
    #[inline]
    pub fn new() -> Self {
        Self {
            port_id: Some(0),
            owner_id: ffi::RTE_ETH_DEV_NO_OWNER.into(),
        }
    }
}

impl Iterator for EthDevIterator {
    type Item = u16;

    fn next(&mut self) -> Option<Self::Item> {
        let port_id = self.port_id.take()?;
        let port_id = unsafe { ffi::rte_eth_find_next_owned_by(port_id, self.owner_id) };

        if port_id == ffi::RTE_MAX_ETHPORTS.into() {
            return None;
        }

        self.port_id = Some(port_id as u16 + 1);
        Some(port_id as u16)
    }
}

/// Get the port ID from device name.
pub fn rte_eth_dev_get_port_by_name(name: &str) -> Result<u16, Error> {
    let name = CString::new(name.to_string()).expect("unexpected '\0'");

    let mut id = 0u16;
    let ec = unsafe { ffi::rte_eth_dev_get_port_by_name(name.as_ptr(), &mut id) };
    if ec == 0 {
        Ok(id)
    } else {
        Err(Error::new(ec))
    }
}
