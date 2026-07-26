fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use a vendored `protoc` so the build does not depend on a system-installed
    // protobuf-compiler (keeps CI and the Docker builds self-contained).
    std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        // Emitted for the gRPC reflection service, which lets tools like
        // `grpcurl` discover the API without a local copy of the proto file.
        .file_descriptor_set_path(out_dir.join("dwd_descriptor.bin"))
        .compile_protos(&["dwdpb/dwd.proto"], &["dwdpb"])?;
    Ok(())
}
