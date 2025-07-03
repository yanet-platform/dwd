use core::ptr::NonNull;

use std::ffi::CString;

use crate::{
    cpu::{CoreId, CoreMask},
    error::Error,
    ffi, rte_xx_free_logging, rte_xx_init_logging,
};

/// EAL (environment abstraction layer) builder.
///
/// https://doc.dpdk.org/guides-17.11/testpmd_app_ug/run_app.html
/// https://doc.dpdk.org/guides/linux_gsg/build_sample_apps.html#running-a-sample-application
#[derive(Debug)]
pub struct EalBuilder {
    args: Vec<CString>,
    ctx: Option<EalLoggingContext>,
}

impl EalBuilder {
    #[inline]
    const fn new() -> Self {
        Self { ctx: None, args: Vec::new() }
    }

    /// Constructs a new EAL builder from the given application name and the
    /// bitmask of the cores to run on..
    pub fn from_coremask(name: String, mask: CoreMask) -> Self {
        Self::new()
            .with_string(name)
            .with_string("-c".into())
            .with_string(format!("0x{:x}", mask.as_u128()))
    }

    /// Core ID that is used as master.
    pub fn with_master_lcore(self, id: CoreId) -> Self {
        self.with_string(format!("--master-lcore={}", id))
    }

    /// Add a PCI device in white list.
    pub fn with_pci_whitelist(self, pci: &str) -> Self {
        self.with_string(format!("--pci-whitelist={}", pci))
    }

    /// Set the type of the current process.
    pub fn with_proc_type(self, ty: &str) -> Self {
        self.with_string(format!("--proc-type={}", ty))
    }

    /// Do not create any shared data structures and run entirely in memory.
    pub fn with_in_memory(self) -> Self {
        self.with_string("--in-memory".into())
    }

    pub fn with_socket_mem(self, mem: usize) -> Self {
        self.with_string(format!("--socket-mem={}", mem))
    }

    /// Capture EAL logs.
    pub fn with_log_capture(mut self) -> Result<Self, Error> {
        self.ctx = Some(EalLoggingContext::new()?);
        Ok(self)
    }

    /// Use malloc instead of hugetlbfs.
    pub fn with_no_huge(self) -> Self {
        self.with_string("--no-huge".into())
    }

    fn with_string(mut self, v: String) -> Self {
        self.args.push(CString::new(v).expect("must not contain '0' byte"));
        self
    }

    pub fn init(self) -> Result<Eal, Error> {
        Eal::new(self.args, self.ctx)
    }
}

#[derive(Debug)]
struct EalLoggingContext {
    fh: NonNull<core::ffi::c_void>,
}

impl EalLoggingContext {
    pub fn new() -> Result<Self, Error> {
        let fh = unsafe { rte_xx_init_logging(do_log) };
        if fh.is_null() {
            return Err(Error::new(ffi::ENOMEM as i32));
        }

        let fh = unsafe { NonNull::new_unchecked(fh) };

        Ok(Self { fh })
    }
}

impl Drop for EalLoggingContext {
    fn drop(&mut self) {
        unsafe { rte_xx_free_logging(self.fh.as_ptr()) };
    }
}

unsafe impl Send for EalLoggingContext {}

#[derive(Debug)]
pub struct Eal {
    _ctx: Option<EalLoggingContext>,
}

impl Eal {
    fn new(args: Vec<CString>, ctx: Option<EalLoggingContext>) -> Result<Self, Error> {
        let mut p_args = args
            .iter()
            .map(|v| v.as_ptr() as *mut core::ffi::c_char)
            .collect::<Vec<_>>();

        let rc = unsafe { ffi::rte_eal_init(p_args.len() as i32, p_args.as_mut_ptr()) };
        if rc == -1 {
            return Err(Error::from_errno());
        }

        Ok(Self { _ctx: ctx })
    }
}

impl Drop for Eal {
    fn drop(&mut self) {
        unsafe { ffi::rte_eal_cleanup() };
    }
}

extern "C" fn do_log(buf: *const core::ffi::c_char, len: usize) {
    let buf = unsafe { core::slice::from_raw_parts(buf as *const u8, len) };
    let buf = core::str::from_utf8(buf).unwrap_or("<UTF8 ERROR>");
    log::debug!("{}", buf.trim_end());
}
