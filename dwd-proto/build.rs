fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use a vendored `protoc` so the build does not depend on a system-installed
    // protobuf-compiler (keeps CI and the Docker builds self-contained).
    std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&["proto/dwd.proto"], &["proto"])?;
    Ok(())
}
