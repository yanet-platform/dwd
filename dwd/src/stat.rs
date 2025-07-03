pub use self::percpu::{
    BurstTxWorkerStat, HttpWorkerStat, PerCpuStat, RxWorkerStat, SockWorkerStat, Stat, TxWorkerStat,
};
use crate::histogram::LogHistogram;

mod percpu;

pub trait CommonStat {
    fn generator(&self) -> u64;
    fn on_generator(&self, v: u64);
}

pub trait TxStat {
    fn num_requests(&self) -> u64;
    fn bytes_tx(&self) -> u64;
}

#[allow(dead_code)]
pub trait BurstTxStat {
    fn num_bursts_tx(&self, idx: usize) -> u64;
}

pub trait RxStat {
    fn num_responses(&self) -> u64;
    fn num_timeouts(&self) -> u64;
    fn bytes_rx(&self) -> u64;
    fn hist(&self) -> LogHistogram;
}

pub trait SocketStat {
    fn num_sock_created(&self) -> u64;
    fn num_sock_errors(&self) -> u64;
    fn num_retransmits(&self) -> u64;
}

pub trait HttpStat {
    fn num_2xx(&self) -> u64;
    fn num_3xx(&self) -> u64;
    fn num_4xx(&self) -> u64;
    fn num_5xx(&self) -> u64;
}
