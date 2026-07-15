use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Self-contained codegen: consumers must not have to supply protoc or wire a
    // crate_universe annotation to build this client. Use the vendored protoc
    // unless the environment already provides one (a Bazel/CI build that passes
    // PROTOC explicitly wins).
    if std::env::var_os("PROTOC").is_none() {
        std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }

    // Resolve the proto from the crate manifest, not the process cwd — this crate
    // is a workspace member two levels below the repo root, and consumers build it
    // from arbitrary working directories (cargo git-dep checkouts, Bazel sandboxes).
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?)
        .join("..")
        .join("..");
    let proto_dir = root.join("proto");
    let proto = proto_dir.join("fastverk/finder/v1/finder.proto");

    println!("cargo:rerun-if-changed={}", proto.display());

    // Same contract as the server, client stubs only.
    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(&[proto], &[proto_dir])?;
    Ok(())
}
