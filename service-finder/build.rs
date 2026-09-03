use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Emit a descriptor set alongside the generated code so main.rs can register
    // gRPC server reflection (grpcurl / k8s debugging).
    let descriptor = PathBuf::from(std::env::var("OUT_DIR")?).join("finder_descriptor.bin");
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .file_descriptor_set_path(descriptor)
        .compile_protos(
            &["proto/fastverk/finder/v1/finder.proto"],
            &["proto"],
        )?;
    Ok(())
}
