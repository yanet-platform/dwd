pub trait CommonStat {
    fn generator(&self) -> u64;
    fn on_generator(&self, v: u64);
}

pub trait TxStat {
    fn num_requests(&self) -> u64;
    fn bytes_tx(&self) -> u64;
}

pub trait SocketStat {
    fn num_sock_created(&self) -> u64;
    fn num_sock_errors(&self) -> u64;
}
