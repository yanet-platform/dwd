#define _GNU_SOURCE
#include <stdio.h>

#include <rte_errno.h>
#include <rte_ethdev.h>
#include <rte_lcore.h>
#include <rte_log.h>
#include <rte_mbuf.h>

int rte_errno_() {
    return rte_errno;
}

struct rte_mbuf* rte_pktmbuf_alloc_(struct rte_mempool *mp) {
    return rte_pktmbuf_alloc(mp);
}

char* rte_pktmbuf_append_(struct rte_mbuf* mbuf, uint16_t len) {
    return rte_pktmbuf_append(mbuf, len);
}

unsigned rte_lcore_id_stub() {
    return rte_lcore_id();
}

uint16_t rte_eth_rx_burst_stub(uint16_t port_id, uint16_t queue_id, struct rte_mbuf **rx_pkts, uint16_t nb_pkts) {
    return rte_eth_rx_burst(port_id, queue_id, rx_pkts, nb_pkts);
}

uint16_t rte_eth_tx_burst_stub(uint16_t port_id, uint16_t queue_id, struct rte_mbuf **tx_pkts, uint16_t nb_pkts) {
    return rte_eth_tx_burst(port_id, queue_id, tx_pkts, nb_pkts);
}

void rte_mbuf_refcnt_set_stub(struct rte_mbuf *m, uint16_t new_value) {
    rte_mbuf_refcnt_set(m, new_value);
}

static ssize_t rte_xx_memfile_write(void* handler, const char* buf, size_t size) {
    ((void(*)(const char*, size_t))handler)(buf, size);
    return size;
}

void* rte_xx_init_logging(void* log) {
    void* fh = NULL;

    cookie_io_functions_t io;
    memset(&io, 0, sizeof(cookie_io_functions_t));
    io.write = rte_xx_memfile_write;

    fh = fopencookie(log, "a+", io);
    int ec = rte_openlog_stream(fh);
    if (ec != 0) {
        return NULL;
    }

    return fh;
}

void rte_xx_free_logging(void* fh) {
    rte_openlog_stream(NULL);
    fclose(fh);
}
