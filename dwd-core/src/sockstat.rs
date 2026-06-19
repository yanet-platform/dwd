use std::os::fd::RawFd;

#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct TcpInfo {
    pub tcpi_state: u8,
    pub tcpi_ca_state: u8,
    pub tcpi_retransmits: u8,
    pub tcpi_probes: u8,
    pub tcpi_backoff: u8,
    pub tcpi_options: u8,
    /// This contains the bitfields `tcpi_snd_wscale` and
    /// `tcpi_rcv_wscale`. Each is 4 bits.
    pub tcpi_snd_rcv_wscale: u8,
    pub tcpi_delivery_rate_app_limited_and_fastopen_client_fail: u8, // 3 bits

    pub tcpi_rto: u32,
    pub tcpi_ato: u32,
    pub tcpi_snd_mss: u32,
    pub tcpi_rcv_mss: u32,

    pub tcpi_unacked: u32,
    pub tcpi_sacked: u32,
    pub tcpi_lost: u32,
    pub tcpi_retrans: u32,
    pub tcpi_fackets: u32,

    // Times
    pub tcpi_last_data_sent: u32,
    pub tcpi_last_ack_sent: u32,
    pub tcpi_last_data_recv: u32,
    pub tcpi_last_ack_recv: u32,

    // Metrics
    pub tcpi_pmtu: u32,
    pub tcpi_rcv_ssthresh: u32,
    pub tcpi_rtt: u32,
    pub tcpi_rttvar: u32,
    pub tcpi_snd_ssthresh: u32,
    pub tcpi_snd_cwnd: u32,
    pub tcpi_advmss: u32,
    pub tcpi_reordering: u32,

    pub tcpi_rcv_rtt: u32,
    pub tcpi_rcv_space: u32,

    /// The number of packets in the current connection that contained data
    /// being retransmitted counted across the lifetime of the connection.
    pub tcpi_total_retrans: u32,

    pub tcpi_pacing_rate: u64,
    pub tcpi_max_pacing_rate: u64,
    pub tcpi_bytes_acked: u64,    // RFC4898 tcpEStatsAppHCThruOctetsAcked
    pub tcpi_bytes_received: u64, // RFC4898 tcpEStatsAppHCThruOctetsReceived
    pub tcpi_segs_out: u32,       // RFC4898 tcpEStatsPerfSegsOut
    pub tcpi_segs_in: u32,        // RFC4898 tcpEStatsPerfSegsIn

    pub tcpi_notsent_bytes: u32,
    pub tcpi_min_rtt: u32,
    pub tcpi_data_segs_in: u32,  // RFC4898 tcpEStatsDataSegsIn
    pub tcpi_data_segs_out: u32, // RFC4898 tcpEStatsDataSegsOut

    pub tcpi_delivery_rate: u64,

    pub tcpi_busy_time: u64,      // Time (usec) busy sending data
    pub tcpi_rwnd_limited: u64,   // Time (usec) limited by receive window
    pub tcpi_sndbuf_limited: u64, // Time (usec) limited by send buffer

    pub tcpi_delivered: u32,
    pub tcpi_delivered_ce: u32,

    pub tcpi_bytes_sent: u64,    // RFC4898 tcpEStatsPerfHCDataOctetsOut
    pub tcpi_bytes_retrans: u64, // RFC4898 tcpEStatsPerfOctetsRetrans
    pub tcpi_dsack_dups: u32,    // RFC4898 tcpEStatsStackDSACKDups
    pub tcpi_reord_seen: u32,    // reordering events seen

    /// Out-of-order packets received.
    pub tcpi_rcv_ooopack: u32,

    /// Peer's advertised receive window after scaling (bytes).
    pub tcpi_snd_wnd: u32,
}

impl TcpInfo {
    #[cfg(target_os = "linux")]
    pub fn new(fd: RawFd) -> Self {
        let mut info = TcpInfo::default();
        let mut len = core::mem::size_of_val(&info) as libc::socklen_t;
        unsafe {
            _ = libc::getsockopt(
                fd,
                libc::SOL_TCP,
                libc::TCP_INFO,
                &mut info as *mut TcpInfo as *mut libc::c_void,
                &mut len as *mut u32,
            );
        }

        info
    }

    #[cfg(not(target_os = "linux"))]
    pub fn new(_fd: RawFd) -> Self {
        Self::default()
    }
}
