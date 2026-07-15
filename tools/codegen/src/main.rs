//! Regenerate `client/rust/src/generated` from the canonical finder proto.
//!
//! Run `cargo run -p codegen` after editing the proto. CI (codegen-drift) runs
//! this and fails if the checked-in output differs — the client ships generated
//! code (no build.rs) so consumers need no protoc/proto/bazel annotation.
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = root.join("client/rust/src/generated");
    std::fs::create_dir_all(&out)?;
    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .out_dir(&out)
        .compile_protos(
            &[root.join("proto/fastverk/finder/v1/finder.proto")],
            &[root.join("proto")],
        )?;
    println!("regenerated {}", out.display());
    Ok(())
}
