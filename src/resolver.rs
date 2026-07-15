//! resolver — the pure capability/selector → endpoints mapping.
//!
//! This is the generalization of botnoc's `web/src/discovery.rs`: instead of a
//! fixed `fastverk.dev/plugin=<id>` label and hard-coded `http`/`grpc` port
//! names, a Service advertises a *capability* (label `finder.fastverk.dev/
//! capability`) and a set of *selector attributes* (annotation
//! `finder.fastverk.dev/selectors`, a JSON object). Resolving is a pure function
//! of the labeled Services in the cache plus the query — so it unit-tests without
//! a cluster (see the tests below, which mirror discovery.rs's).

use std::collections::{BTreeMap, HashMap};

use k8s_openapi::api::core::v1::Service;
use kube::ResourceExt;

use crate::pb::Endpoint;

/// Label whose value is the capability a Service serves (the List filter key).
pub const CAPABILITY_LABEL: &str = "finder.fastverk.dev/capability";
/// Annotation holding a JSON object of advertised selector attributes, e.g.
/// `{"ext":[".rs",".rlib"],"language":"rust"}`. A value is a string or a string
/// array. Absent ⇒ the Service matches only the empty selector.
pub const SELECTORS_ANNOTATION: &str = "finder.fastverk.dev/selectors";
/// Annotation pinning an explicit endpoint URL (for external / non-Service
/// targets). When present it wins over named-port derivation.
pub const ENDPOINT_ANNOTATION: &str = "finder.fastverk.dev/endpoint";

/// Advertised selector attributes: key → one-or-more values.
type Advertised = BTreeMap<String, Vec<String>>;

/// Parse the selectors annotation JSON into `key → values`. Tolerant: a missing
/// or malformed annotation yields an empty map (the Service then matches only the
/// empty selector), never an error — resolution must not fail on one bad CR.
fn parse_selectors(raw: Option<&String>) -> Advertised {
    let mut out = Advertised::new();
    let Some(raw) = raw else { return out };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) else {
        return out;
    };
    let Some(obj) = val.as_object() else { return out };
    for (k, v) in obj {
        let values = match v {
            serde_json::Value::String(s) => vec![s.clone()],
            serde_json::Value::Array(arr) => arr
                .iter()
                .filter_map(|e| e.as_str().map(|s| s.to_string()))
                .collect(),
            // numbers/bools coerced to their string form so `{"port":50060}` works
            serde_json::Value::Number(n) => vec![n.to_string()],
            serde_json::Value::Bool(b) => vec![b.to_string()],
            _ => continue,
        };
        if !values.is_empty() {
            out.insert(k.clone(), values);
        }
    }
    out
}

/// A Service matches a query when, for EVERY key in the query, the Service
/// advertises that key and the query value is among its advertised values. An
/// empty query matches every Service carrying the capability.
fn selector_matches(query: &BTreeMap<String, String>, advertised: &Advertised) -> bool {
    query.iter().all(|(k, want)| {
        advertised
            .get(k)
            .is_some_and(|vals| vals.iter().any(|v| v == want))
    })
}

/// Flatten advertised attributes to `key → value` for the response (arrays joined
/// with ',') so a consumer can distinguish multiple resolved backends. `HashMap`
/// to match the proto `map<string,string>` field type.
fn flatten(advertised: &Advertised) -> HashMap<String, String> {
    advertised
        .iter()
        .map(|(k, v)| (k.clone(), v.join(",")))
        .collect()
}

/// Map ONE Service to its matching endpoints for `(capability, query, port_name)`.
/// Returns empty when the Service doesn't carry the capability, doesn't match the
/// selector, or exposes no usable port. Pure — the unit tests drive it directly.
pub fn service_endpoints(
    svc: &Service,
    ns: &str,
    capability: &str,
    query: &BTreeMap<String, String>,
    port_name: &str,
) -> Vec<Endpoint> {
    // Capability gate (the cache is label-filtered, but re-check for safety +
    // to support a value-less "label exists" watch feeding multiple capabilities).
    if svc.labels().get(CAPABILITY_LABEL).map(String::as_str) != Some(capability) {
        return Vec::new();
    }
    let advertised = parse_selectors(svc.annotations().get(SELECTORS_ANNOTATION));
    if !selector_matches(query, &advertised) {
        return Vec::new();
    }
    let Some(name) = svc.metadata.name.clone() else {
        return Vec::new();
    };
    let attributes = flatten(&advertised);

    // Explicit endpoint override (external / non-Service target).
    if let Some(url) = svc.annotations().get(ENDPOINT_ANNOTATION) {
        if port_name.is_empty() {
            return vec![Endpoint {
                url: url.clone(),
                port_name: String::new(),
                attributes,
                service: name,
                namespace: ns.to_string(),
            }];
        }
        // A port_name was requested but this Service only pins a raw URL — it
        // can't satisfy a named-port query, so it contributes nothing.
        return Vec::new();
    }

    // Derive from named ports → cluster DNS (mirrors discovery.rs).
    let ports = svc.spec.as_ref().and_then(|s| s.ports.as_ref());
    let Some(ports) = ports else { return Vec::new() };
    ports
        .iter()
        .filter(|p| p.name.is_some())
        .filter(|p| port_name.is_empty() || p.name.as_deref() == Some(port_name))
        .map(|p| Endpoint {
            url: format!("http://{name}.{ns}.svc.cluster.local:{}", p.port),
            port_name: p.name.clone().unwrap_or_default(),
            attributes: attributes.clone(),
            service: name.clone(),
            namespace: ns.to_string(),
        })
        .collect()
}

/// Resolve across all Services in the cache snapshot (pure over the given set).
pub fn resolve<'a>(
    services: impl IntoIterator<Item = &'a Service>,
    ns: &str,
    capability: &str,
    query: &BTreeMap<String, String>,
    port_name: &str,
) -> Vec<Endpoint> {
    let mut out = Vec::new();
    for svc in services {
        out.extend(service_endpoints(svc, ns, capability, query, port_name));
    }
    // Stable order (by url) so Watch can dedupe unchanged snapshots cheaply.
    out.sort_by(|a, b| a.url.cmp(&b.url));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{ServicePort, ServiceSpec};
    use std::collections::BTreeMap;

    fn svc(
        name: &str,
        capability: Option<&str>,
        selectors_json: Option<&str>,
        endpoint: Option<&str>,
        ports: &[(&str, i32)],
    ) -> Service {
        let mut s = Service::default();
        s.metadata.name = Some(name.to_string());
        let mut labels = BTreeMap::new();
        if let Some(c) = capability {
            labels.insert(CAPABILITY_LABEL.to_string(), c.to_string());
        }
        s.metadata.labels = Some(labels);
        let mut ann = BTreeMap::new();
        if let Some(j) = selectors_json {
            ann.insert(SELECTORS_ANNOTATION.to_string(), j.to_string());
        }
        if let Some(e) = endpoint {
            ann.insert(ENDPOINT_ANNOTATION.to_string(), e.to_string());
        }
        s.metadata.annotations = Some(ann);
        s.spec = Some(ServiceSpec {
            ports: Some(
                ports
                    .iter()
                    .map(|(n, port)| ServicePort {
                        name: Some(n.to_string()),
                        port: *port,
                        ..Default::default()
                    })
                    .collect(),
            ),
            ..Default::default()
        });
        s
    }

    fn q(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn resolves_parser_by_extension() {
        let services = vec![
            svc(
                "parser-rust",
                Some("ast-parser"),
                Some(r#"{"ext":[".rs",".rlib"],"language":"rust"}"#),
                None,
                &[("grpc", 50060)],
            ),
            svc(
                "parser-python",
                Some("ast-parser"),
                Some(r#"{"ext":[".py"],"language":"python"}"#),
                None,
                &[("grpc", 50060)],
            ),
        ];
        let out = resolve(&services, "fastverk", "ast-parser", &q(&[("ext", ".rs")]), "grpc");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].url,
            "http://parser-rust.fastverk.svc.cluster.local:50060"
        );
        assert_eq!(out[0].port_name, "grpc");
        assert_eq!(out[0].attributes.get("language").unwrap(), "rust");
    }

    #[test]
    fn empty_selector_matches_all_in_capability() {
        // The console-plugin case: no selector, resolve every plugin's http port.
        let services = vec![
            svc("plugin-forge", Some("console-plugin"), None, None, &[("http", 8080), ("grpc", 50053)]),
            svc("plugin-depot", Some("console-plugin"), None, None, &[("http", 8080)]),
            svc("unrelated", Some("ast-parser"), None, None, &[("http", 9000)]),
        ];
        let out = resolve(&services, "fastverk", "console-plugin", &BTreeMap::new(), "http");
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|e| e.port_name == "http"));
    }

    #[test]
    fn selector_miss_yields_nothing() {
        let services = vec![svc(
            "parser-rust",
            Some("ast-parser"),
            Some(r#"{"ext":[".rs"]}"#),
            None,
            &[("grpc", 50060)],
        )];
        let out = resolve(&services, "fastverk", "ast-parser", &q(&[("ext", ".go")]), "grpc");
        assert!(out.is_empty());
    }

    #[test]
    fn wrong_capability_skipped() {
        let services = vec![svc("x", Some("graphd"), None, None, &[("grpc", 50051)])];
        assert!(resolve(&services, "ns", "ast-parser", &BTreeMap::new(), "").is_empty());
    }

    #[test]
    fn endpoint_override_wins_over_ports() {
        let services = vec![svc(
            "external-thing",
            Some("graphd"),
            Some(r#"{"repo":"fastverk/botnoc"}"#),
            Some("https://graphd.example.com:443"),
            &[("grpc", 50051)],
        )];
        // No port_name → the override is returned.
        let out = resolve(&services, "ns", "graphd", &q(&[("repo", "fastverk/botnoc")]), "");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://graphd.example.com:443");
        assert_eq!(out[0].port_name, "");
        // A named-port query can't be satisfied by a raw-URL override.
        assert!(resolve(&services, "ns", "graphd", &BTreeMap::new(), "grpc").is_empty());
    }

    #[test]
    fn all_named_ports_when_port_name_empty() {
        let services = vec![svc(
            "plugin-forge",
            Some("console-plugin"),
            None,
            None,
            &[("http", 8080), ("grpc", 50053)],
        )];
        let out = resolve(&services, "fastverk", "console-plugin", &BTreeMap::new(), "");
        assert_eq!(out.len(), 2); // both named ports
    }

    #[test]
    fn multi_key_selector_requires_all() {
        let services = vec![svc(
            "parser-rust",
            Some("ast-parser"),
            Some(r#"{"ext":[".rs"],"language":"rust"}"#),
            None,
            &[("grpc", 50060)],
        )];
        // both keys satisfied
        assert_eq!(
            resolve(&services, "ns", "ast-parser", &q(&[("ext", ".rs"), ("language", "rust")]), "grpc").len(),
            1
        );
        // one key wrong → no match
        assert!(resolve(
            &services,
            "ns",
            "ast-parser",
            &q(&[("ext", ".rs"), ("language", "go")]),
            "grpc"
        )
        .is_empty());
    }
}
