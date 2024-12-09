#[cfg(target_os = "linux")]
fn main() {
    use std::{env, path::PathBuf};

    use cc::Build;

    /// Path where DPDK includes are located.
    // const DPDK_INCLUDE_PATHS: &[&str] = &["/usr/local/include"];
    const DPDK_INCLUDE_PATHS: &[&str] = &[
        "/usr/include/dpdk",
        "/usr/include/x86_64-linux-gnu/dpdk",
        "/usr/share/dpdk/x86_64-sandybridge-linuxapp-gcc/include",
    ];

    // const DPDK_LIBRARY_PATHS: &[&str] = &["/usr/local/lib64/dpdk/pmds-25.0"];
    const DPDK_LIBRARY_PATHS: &[&str] = &["/usr/share/dpdk/x86_64-sandybridge-linuxapp-gcc"];

    /// DPDK core libraries to link with (static).
    // const RTE_CORE_LIBS: &[&str] = &[
    //     "rte_bus_auxiliary",
    //     "rte_bus_pci",
    //     "rte_cryptodev",
    //     "rte_eal",
    //     "rte_ethdev",
    //     "rte_hash",
    //     "rte_kvargs",
    //     "rte_log",
    //     "rte_mbuf",
    //     "rte_mempool",
    //     "rte_mempool_ring",
    //     "rte_net",
    //     "rte_pci",
    //     "rte_rcu",
    //     "rte_ring",
    //     "rte_telemetry",
    // ];
    const RTE_CORE_LIBS: &[&str] = &[
        "rte_bus_pci",
        "rte_eal",
        "rte_ethdev",
        "rte_kvargs",
        "rte_mbuf",
        "rte_mempool",
        "rte_mempool_ring",
        "rte_net",
        "rte_pci",
        "rte_ring",
    ];

    // PMD (poll-mode-driver) libraries dependency.
    // const RTE_PMD_LIBS: &[&str] = &["rte_common_mlx5", "rte_crypto_mlx5", "rte_net_mlx5"];
    const RTE_PMD_LIBS: &[&str] = &["rte_pmd_mlx5"];

    // Additional dependencies, which must be linked dynamically against, because
    // of license.
    const RTE_DEPS_LIBS: &[&str] = &["numa", "ibverbs", "mlx5"];

    println!("cargo:rerun-if-changed=src/ffi/wrapper.h");
    println!("cargo:rerun-if-changed=src/ffi/stub.c");

    println!("cargo:rustc-link-search=/usr/lib/x86_64-linux-gnu/");
    for path in DPDK_LIBRARY_PATHS {
        println!("cargo:rustc-link-search={path}/lib");
    }

    // https://stackoverflow.com/questions/1202494/why-doesnt-attribute-constructor-work-in-a-static-library
    for lib in RTE_CORE_LIBS {
        println!("cargo:rustc-link-lib=static:+whole-archive,-bundle={}", lib);
    }
    for lib in RTE_PMD_LIBS {
        println!("cargo:rustc-link-lib=static:+whole-archive,-bundle={}", lib);
    }
    for lib in RTE_DEPS_LIBS {
        println!("cargo:rustc-link-lib=dylib={}", lib)
    }

    let bindings = bindgen::Builder::default()
        .header("src/ffi/wrapper.h")
        // Invalidate the built crate whenever any of the included header files
        // changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate_comments(false)
        .generate_inline_functions(true)
        // Treat as opaque as per issue without combining align/packed:
        //   https://github.com/rust-lang/rust-bindgen/issues/1538
        .opaque_type(r"rte_arp_ipv4")
        .opaque_type(r"rte_arp_hdr")
        .opaque_type(r"rte_l2tpv2_combined_msg_hdr")
        .allowlist_type(r"rte_.*")
        .allowlist_type(r"eth_.*")
        .blocklist_type(r"rte_gtp_.*")
        .allowlist_function(r"rte_.*")
        .allowlist_function(r"eth_.*")
        .allowlist_var(r".*")
        .blocklist_item(r"rte_flow_item_gtp.*")
        .derive_copy(true)
        .derive_debug(true)
        .derive_default(true)
        .derive_partialeq(true)
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        .clang_args(DPDK_INCLUDE_PATHS.iter().map(|v| format!("-I{v}")))
        .clang_arg("-finline-functions")
        .clang_arg("-march=native")
        .generate()
        .unwrap();

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    bindings.write_to_file(out_dir.join("bindings.rs")).unwrap();

    // Step 3: Compile a stub file so Rust can access `inline` functions in the
    // headers that aren't compiled into the libraries.
    let mut builder: Build = cc::Build::new();
    builder.opt_level(3);
    builder.pic(true);
    builder.flag("-march=native");
    builder.file("src/ffi/stub.c");
    builder.includes(DPDK_INCLUDE_PATHS.iter());
    builder.compile("stub");
}

#[cfg(not(target_os = "linux"))]
fn main() {}
