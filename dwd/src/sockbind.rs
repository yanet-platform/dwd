use core::{
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::atomic::{AtomicUsize, Ordering},
};
use std::sync::Arc;

use crate::Produce;

#[derive(Debug)]
pub struct SocketAddrGen {
    addrs: Vec<SocketAddr>,
    idx: AtomicUsize,
}

impl SocketAddrGen {
    #[inline]
    pub const fn new(addrs: Vec<SocketAddr>) -> Self {
        Self { addrs, idx: AtomicUsize::new(0) }
    }

    #[inline]
    pub fn unspecified6() -> Self {
        Self::new(vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)])
    }
}

impl Produce for Arc<SocketAddrGen> {
    type Item = SocketAddr;

    #[inline]
    fn next(&self) -> &Self::Item {
        let idx = self.idx.fetch_add(1, Ordering::Relaxed);
        &self.addrs[idx % self.addrs.len()]
    }
}
