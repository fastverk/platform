fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Same contract as the server, client stubs only. The proto lives at the repo
    // root; this crate is a workspace member two levels down.
    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(
            &["../../proto/fastverk/finder/v1/finder.proto"],
            &["../../proto"],
        )?;
    Ok(())
}
