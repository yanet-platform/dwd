use std::ffi::CString;

use crate::{error::Error, ffi};

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
