// Compile forge.v1 from fastverk/contracts, including guarded provisioning.
// Never fall back to stale local proto copies when the external root is absent.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=FORGE_PROTO_FILE");
    println!("cargo:rerun-if-env-changed=FASTVERK_CONTRACTS_PROTO_DIR");
    let root = match std::env::var("FORGE_PROTO_FILE") {
        Ok(file) => file.strip_suffix("/forge/v1/forge.proto")
            .ok_or("FORGE_PROTO_FILE must end with /forge/v1/forge.proto")?.to_owned(),
        Err(_) => std::env::var("FASTVERK_CONTRACTS_PROTO_DIR")
            .map_err(|_| "Cargo requires FASTVERK_CONTRACTS_PROTO_DIR pointing to the pinned fastverk/contracts proto directory")?,
    };
    let files: Vec<_> = [
        "forge",
        "provision",
        "discovery",
        "events",
        "guarded_provision",
    ]
    .iter()
    .map(|name| format!("{root}/forge/v1/{name}.proto"))
    .collect();
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&files, &[root])?;
    Ok(())
}
