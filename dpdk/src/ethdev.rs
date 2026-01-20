use std::ffi::CString;

use crate::{error::Error, ffi};

// RSS hash type constants - these are macros in DPDK headers (not exported by bindgen).
// Defined as RTE_BIT64(N) in rte_ethdev.h.
pub const RTE_ETH_RSS_IPV4: u64 = 1 << 2;
pub const RTE_ETH_RSS_FRAG_IPV4: u64 = 1 << 3;
pub const RTE_ETH_RSS_NONFRAG_IPV4_TCP: u64 = 1 << 4;
pub const RTE_ETH_RSS_NONFRAG_IPV4_UDP: u64 = 1 << 5;
pub const RTE_ETH_RSS_NONFRAG_IPV4_SCTP: u64 = 1 << 6;
pub const RTE_ETH_RSS_NONFRAG_IPV4_OTHER: u64 = 1 << 7;
pub const RTE_ETH_RSS_IPV6: u64 = 1 << 8;
pub const RTE_ETH_RSS_FRAG_IPV6: u64 = 1 << 9;
pub const RTE_ETH_RSS_NONFRAG_IPV6_TCP: u64 = 1 << 10;
pub const RTE_ETH_RSS_NONFRAG_IPV6_UDP: u64 = 1 << 11;
pub const RTE_ETH_RSS_NONFRAG_IPV6_SCTP: u64 = 1 << 12;
pub const RTE_ETH_RSS_NONFRAG_IPV6_OTHER: u64 = 1 << 13;
pub const RTE_ETH_RSS_IPV6_EX: u64 = 1 << 16;

/// RSS hash types combined for IP (both IPv4 and IPv6).
/// This is equivalent to RTE_ETH_RSS_IP macro in DPDK headers.
pub const RTE_ETH_RSS_IP: u64 = RTE_ETH_RSS_IPV4
    | RTE_ETH_RSS_FRAG_IPV4
    | RTE_ETH_RSS_NONFRAG_IPV4_OTHER
    | RTE_ETH_RSS_IPV6
    | RTE_ETH_RSS_FRAG_IPV6
    | RTE_ETH_RSS_NONFRAG_IPV6_OTHER
    | RTE_ETH_RSS_IPV6_EX;

// RX offload flags - these are macros in DPDK headers (not exported by bindgen).
// Defined as RTE_BIT64(N) in rte_ethdev.h.
pub const RTE_ETH_RX_OFFLOAD_VLAN_STRIP: u64 = 1 << 0;
pub const RTE_ETH_RX_OFFLOAD_IPV4_CKSUM: u64 = 1 << 1;
pub const RTE_ETH_RX_OFFLOAD_UDP_CKSUM: u64 = 1 << 2;
pub const RTE_ETH_RX_OFFLOAD_TCP_CKSUM: u64 = 1 << 3;
pub const RTE_ETH_RX_OFFLOAD_TCP_LRO: u64 = 1 << 4;
pub const RTE_ETH_RX_OFFLOAD_QINQ_STRIP: u64 = 1 << 5;
pub const RTE_ETH_RX_OFFLOAD_OUTER_IPV4_CKSUM: u64 = 1 << 6;
pub const RTE_ETH_RX_OFFLOAD_MACSEC_STRIP: u64 = 1 << 7;
pub const RTE_ETH_RX_OFFLOAD_VLAN_FILTER: u64 = 1 << 9;
pub const RTE_ETH_RX_OFFLOAD_VLAN_EXTEND: u64 = 1 << 10;
pub const RTE_ETH_RX_OFFLOAD_SCATTER: u64 = 1 << 13;

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

impl Default for EthDevIterator {
    fn default() -> Self {
        Self::new()
    }
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
