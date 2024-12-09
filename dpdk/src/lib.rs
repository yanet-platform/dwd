#![cfg(target_os = "linux")]

use cpu::CoreId;

pub mod boxed;
pub mod cpu;
pub mod eal;
pub mod error;
pub mod ethdev;

pub const RTE_CACHE_LINE_SIZE: u32 = ffi::RTE_CACHE_LINE_SIZE;

pub type SocketId = u32;

pub mod ffi {
    #![allow(non_upper_case_globals)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(unused)]

    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

#[link(name = "stub")]
extern "C" {
    // fn rte_pktmbuf_free_(packet: *mut rte_mbuf);
    fn rte_pktmbuf_alloc_(mp: *mut ffi::rte_mempool) -> *mut ffi::rte_mbuf;
    fn rte_pktmbuf_append_(mbuf: *mut ffi::rte_mbuf, len: core::ffi::c_ushort) -> *mut core::ffi::c_char;
    fn rte_eth_tx_burst_stub(port_id: u16, queue_id: u16, tx_pkts: *mut *mut ffi::rte_mbuf, nb_pkts: u16) -> u16;
    fn rte_eth_rx_burst_stub(port_id: u16, queue_id: u16, rx_pkts: *mut *mut ffi::rte_mbuf, nb_pkts: u16) -> u16;
    // fn rte_mbuf_refcnt_read_(m: *const rte_mbuf) -> u16;
    // fn rte_mbuf_refcnt_update_(m: *mut rte_mbuf, value: i16) -> u16;
    // fn rte_pktmbuf_adj_(packet: *mut rte_mbuf, len: u16) -> *mut c_char;
    // fn rte_pktmbuf_trim_(packet: *mut rte_mbuf, len: u16) -> c_int;
    // fn rte_pktmbuf_headroom_(m: *const rte_mbuf) -> u16;
    // fn rte_pktmbuf_tailroom_(m: *const rte_mbuf) -> u16;
    fn rte_errno_() -> core::ffi::c_int;
    // fn rte_pktmbuf_chain_(head: *mut rte_mbuf, tail: *mut rte_mbuf) -> c_int;
    fn rte_lcore_id_stub() -> core::ffi::c_uint;
    fn rte_mbuf_refcnt_set_stub(mbuf: *mut ffi::rte_mbuf, new_value: u16);
    fn rte_xx_init_logging(data: extern "C" fn(buf: *const core::ffi::c_char, len: usize)) -> *mut core::ffi::c_void;
    fn rte_xx_free_logging(fh: *mut core::ffi::c_void);
}

pub fn rte_lcore_to_socket_id(id: CoreId) -> SocketId {
    unsafe { ffi::rte_lcore_to_socket_id(id.as_u16() as SocketId) }
}

/// Returns the error number value, stored per-thread, which can be queried
/// after calls to certain functions to determine why those functions failed.
///
/// Uses standard values from errno.h wherever possible, with a small number
/// of additional possible values for RTE-specific conditions.
#[inline]
pub unsafe fn rte_errno() -> core::ffi::c_int {
    rte_errno_()
}

#[inline]
pub unsafe fn rte_pktmbuf_alloc(mp: *mut ffi::rte_mempool) -> *mut ffi::rte_mbuf {
    rte_pktmbuf_alloc_(mp)
}

#[inline]
pub unsafe fn rte_pktmbuf_append(mbuf: *mut ffi::rte_mbuf, len: core::ffi::c_ushort) -> *mut core::ffi::c_char {
    rte_pktmbuf_append_(mbuf, len)
}

#[inline]
pub unsafe fn rte_eth_rx_burst(port_id: u16, queue_id: u16, rx_pkts: *mut *mut ffi::rte_mbuf, nb_pkts: u16) -> u16 {
    rte_eth_rx_burst_stub(port_id, queue_id, rx_pkts, nb_pkts)
}

#[inline]
pub unsafe fn rte_eth_tx_burst(port_id: u16, queue_id: u16, tx_pkts: *mut *mut ffi::rte_mbuf, nb_pkts: u16) -> u16 {
    rte_eth_tx_burst_stub(port_id, queue_id, tx_pkts, nb_pkts)
}

#[inline]
pub fn rte_lcore_id() -> CoreId {
    let id = unsafe { rte_lcore_id_stub() } as u16;
    CoreId::new(id)
}

#[inline]
pub unsafe fn rte_mbuf_refcnt_set(mbuf: *mut ffi::rte_mbuf, new_value: u16) {
    rte_mbuf_refcnt_set_stub(mbuf, new_value);
}
