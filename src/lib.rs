//! service-finder — semantic / capability-based service discovery for fastverk.
//!
//! One `Resolve(capability, selector) -> [endpoints]` (+ `Watch`) that replaces
//! the ~6 hand-rolled "list k8s Services/CRs by an attribute, read the endpoint,
//! synthesize cluster DNS" loops across the platform (botnoc's discovery.rs, the
//! mcp-catalog, controlplane rbe.rs, the modgraph precompute, the LanguageParser
//! parser registry, …). Backed by capability-labeled k8s Services; consumers stay
//! k8s-agnostic. See `proto/fastverk/finder/v1/finder.proto` for the contract and
//! the registration convention.

/// Generated `fastverk.finder.v1` types + gRPC server/client stubs.
pub mod pb {
    tonic::include_proto!("fastverk.finder.v1");
}

/// The proto file-descriptor set, for gRPC server reflection (grpcurl et al.).
pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("finder_descriptor");

pub mod groups;
pub mod registry;
pub mod resolver;
pub mod service;
