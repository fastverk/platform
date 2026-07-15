//! service-finder-server — the finder daemon.
//!
//! Serves `fastverk.finder.v1.Finder` on `:50060` (override `FINDER_ADDR`),
//! backed by a live cache of Services labeled `finder.fastverk.dev/capability`
//! in `POD_NAMESPACE` (default: the client's default namespace). Needs only
//! `list`/`watch` on `services` (see the chart's Role). gRPC health + reflection
//! are registered so k8s grpc probes and `grpcurl` work out of the box.

use clap::Parser;
use tonic::transport::Server;
use tonic_health::server::health_reporter;

use service_finder::pb::finder_server::FinderServer;
use service_finder::registry::Registry;
use service_finder::service::FinderService;

#[derive(Parser, Debug)]
#[command(name = "service-finder-server")]
struct Args {
    /// gRPC listen address.
    #[arg(long, env = "FINDER_ADDR", default_value = "0.0.0.0:50060")]
    addr: String,
    /// Namespace to discover Services in. Defaults to POD_NAMESPACE, then the
    /// kube client's default namespace.
    #[arg(long, env = "POD_NAMESPACE")]
    namespace: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // rustls (kube's TLS) needs a crypto provider installed once, like every
    // other fastverk service.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "service_finder=info,info".into()),
        )
        .init();

    let args = Args::parse();
    let client = kube::Client::try_default().await?;
    let ns = args
        .namespace
        .unwrap_or_else(|| client.default_namespace().to_string());

    let registry = Registry::spawn(client, ns).await?;
    let finder = FinderService::new(registry);

    // Mark the Finder service SERVING for k8s grpc health probes.
    let (mut health, health_service) = health_reporter();
    health.set_serving::<FinderServer<FinderService>>().await;

    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(service_finder::FILE_DESCRIPTOR_SET)
        .build_v1()?;

    let addr = args.addr.parse()?;
    tracing::info!(%addr, "service-finder serving fastverk.finder.v1.Finder");

    Server::builder()
        .add_service(health_service)
        .add_service(reflection)
        .add_service(FinderServer::new(finder))
        .serve(addr)
        .await?;

    Ok(())
}
