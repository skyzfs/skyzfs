//! CSI v1 codegen. The vendored protoc keeps the build self-contained —
//! no system protobuf-compiler needed on CI runners or in Docker builds.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("PROTOC").is_none() {
        std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }
    let wkt_include = protoc_bin_vendored::include_path()?;
    tonic_build::configure()
        .build_client(false)
        .compile_protos(
            &["proto/csi.proto"],
            &[std::path::PathBuf::from("proto"), wkt_include],
        )?;
    Ok(())
}
