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

/// A legacy label the finder treats AS a capability, so Services that already
/// carry a domain label become discoverable without being re-stamped. E.g.
/// `fastverk.dev/plugin=forge` ⇒ capability "console-plugin" with an injected
/// selector `{plugin: "forge"}` — so the whole console plugin fleet is resolvable
/// via `Resolve("console-plugin", {plugin: id})` with zero plugin-chart changes.
#[derive(Clone, Debug)]
pub struct LabelAlias {
    /// The existing label key on the Service (e.g. "fastverk.dev/plugin").
    pub label: String,
    /// The capability it maps to (e.g. "console-plugin").
    pub capability: String,
    /// The selector attribute the label's value is injected as (e.g. "plugin").
    pub selector_key: String,
}

/// Built-in aliases. The console plugin fleet already labels its Services
/// `fastverk.dev/plugin=<id>` (via each chart's `_helpers.tpl`), so the finder
/// serves them as `console-plugin` out of the box.
pub fn default_aliases() -> Vec<LabelAlias> {
    vec![LabelAlias {
        label: "fastverk.dev/plugin".to_string(),
        capability: "console-plugin".to_string(),
        selector_key: "plugin".to_string(),
    }]
}

/// Does `svc` serve `capability` — directly via the capability label, or via an
/// alias label? Returns the extra selector attributes an alias injects (empty for
/// a direct match), or None if the Service doesn't serve the capability at all.
fn capability_extra(svc: &Service, aliases: &[LabelAlias], capability: &str) -> Option<Advertised> {
    if svc.labels().get(CAPABILITY_LABEL).map(String::as_str) == Some(capability) {
        return Some(Advertised::new());
    }
    for a in aliases {
        if a.capability != capability {
            continue;
        }
        if let Some(v) = svc.labels().get(&a.label) {
            let mut extra = Advertised::new();
            extra.insert(a.selector_key.clone(), vec![v.clone()]);
            return Some(extra);
        }
    }
    None
}

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
    aliases: &[LabelAlias],
    capability: &str,
    query: &BTreeMap<String, String>,
    port_name: &str,
) -> Vec<Endpoint> {
    // Capability gate: served directly (finder.fastverk.dev/capability) or via a
    // legacy-label alias (which injects an extra selector, e.g. {plugin: forge}).
    let Some(extra) = capability_extra(svc, aliases, capability) else {
        return Vec::new();
    };
    let mut advertised = parse_selectors(svc.annotations().get(SELECTORS_ANNOTATION));
    for (k, v) in extra {
        advertised.entry(k).or_insert(v);
    }
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
    aliases: &[LabelAlias],
    capability: &str,
    query: &BTreeMap<String, String>,
    port_name: &str,
) -> Vec<Endpoint> {
    let mut out = Vec::new();
    for svc in services {
        out.extend(service_endpoints(svc, ns, aliases, capability, query, port_name));
    }
    // Stable order (by url) so Watch can dedupe unchanged snapshots cheaply.
    out.sort_by(|a, b| a.url.cmp(&b.url));
    out
}

/// Annotation on a backing Service giving its ordering priority (ascending int;
/// lower = earlier). Read by EndpointGroups with Ordering::Priority.
pub const PRIORITY_ANNOTATION: &str = "finder.fastverk.dev/priority";

/// How an EndpointGroup orders its resolved endpoints. Ordering is POLICY (it
/// lives in the group CR); the finder applies it at resolve time — endpoints are
/// never materialized into etcd.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Ordering {
    /// Stable by url (the flat `resolve` default) — no primary/fallback notion.
    #[default]
    Unordered,
    /// Ascending integer read from each backing Service's priority annotation
    /// (missing ⇒ last); ties broken by url. Gives primary→fallback / canary.
    Priority,
}

/// Order a resolved endpoint set per `ordering`. `priority_of` supplies each
/// endpoint's priority (from its backing Service's annotation) — injected so this
/// stays pure and unit-testable. Returns a new ordered Vec.
pub fn order_endpoints(
    mut endpoints: Vec<Endpoint>,
    ordering: &Ordering,
    priority_of: impl Fn(&Endpoint) -> i64,
) -> Vec<Endpoint> {
    match ordering {
        Ordering::Unordered => endpoints.sort_by(|a, b| a.url.cmp(&b.url)),
        Ordering::Priority => {
            endpoints.sort_by(|a, b| {
                priority_of(a)
                    .cmp(&priority_of(b))
                    .then_with(|| a.url.cmp(&b.url))
            });
        }
    }
    endpoints
}

/// Parse a Service's priority annotation to an int (missing/invalid ⇒ i64::MAX so
/// it sorts last).
pub fn priority_of_service(svc: &Service) -> i64 {
    svc.annotations()
        .get(PRIORITY_ANNOTATION)
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(i64::MAX)
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
        let out = resolve(&services, "fastverk", &[], "ast-parser", &q(&[("ext", ".rs")]), "grpc");
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
        let out = resolve(&services, "fastverk", &[], "console-plugin", &BTreeMap::new(), "http");
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
        let out = resolve(&services, "fastverk", &[], "ast-parser", &q(&[("ext", ".go")]), "grpc");
        assert!(out.is_empty());
    }

    #[test]
    fn wrong_capability_skipped() {
        let services = vec![svc("x", Some("graphd"), None, None, &[("grpc", 50051)])];
        assert!(resolve(&services, "ns", &[], "ast-parser", &BTreeMap::new(), "").is_empty());
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
        let out = resolve(&services, "ns", &[], "graphd", &q(&[("repo", "fastverk/botnoc")]), "");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://graphd.example.com:443");
        assert_eq!(out[0].port_name, "");
        // A named-port query can't be satisfied by a raw-URL override.
        assert!(resolve(&services, "ns", &[], "graphd", &BTreeMap::new(), "grpc").is_empty());
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
        let out = resolve(&services, "fastverk", &[], "console-plugin", &BTreeMap::new(), "");
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
            resolve(&services, "ns", &[], "ast-parser", &q(&[("ext", ".rs"), ("language", "rust")]), "grpc").len(),
            1
        );
        // one key wrong → no match
        assert!(resolve(
            &services,
            "ns",
            &[],
            "ast-parser",
            &q(&[("ext", ".rs"), ("language", "go")]),
            "grpc"
        )
        .is_empty());
    }

    #[test]
    fn label_alias_makes_plugin_fleet_discoverable() {
        // A plugin Service carries only the legacy `fastverk.dev/plugin` label —
        // no finder.fastverk.dev/* — yet resolves as capability "console-plugin"
        // with an injected {plugin: <id>} selector, via the default alias.
        let mut s = Service::default();
        s.metadata.name = Some("plugin-forge".to_string());
        let mut labels = BTreeMap::new();
        labels.insert("fastverk.dev/plugin".to_string(), "forge".to_string());
        s.metadata.labels = Some(labels);
        s.spec = Some(ServiceSpec {
            ports: Some(vec![ServicePort {
                name: Some("http".to_string()),
                port: 8080,
                ..Default::default()
            }]),
            ..Default::default()
        });
        let services = vec![s];
        let aliases = default_aliases();

        // resolves the whole fleet (empty selector)
        let all = resolve(&services, "fastverk", &aliases, "console-plugin", &BTreeMap::new(), "http");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].url, "http://plugin-forge.fastverk.svc.cluster.local:8080");
        assert_eq!(all[0].attributes.get("plugin").unwrap(), "forge");

        // resolves a specific plugin by the injected selector
        let one = resolve(&services, "fastverk", &aliases, "console-plugin", &q(&[("plugin", "forge")]), "http");
        assert_eq!(one.len(), 1);
        // wrong plugin id → nothing
        assert!(resolve(&services, "fastverk", &aliases, "console-plugin", &q(&[("plugin", "depot")]), "http").is_empty());
        // without the alias, the legacy label is invisible
        assert!(resolve(&services, "fastverk", &[], "console-plugin", &BTreeMap::new(), "http").is_empty());
    }

    #[test]
    fn priority_ordering_puts_lowest_first() {
        let eps = vec![
            Endpoint { url: "http://fallback:1".into(), ..Default::default() },
            Endpoint { url: "http://primary:1".into(), ..Default::default() },
            Endpoint { url: "http://nopri:1".into(), ..Default::default() },
        ];
        // primary=0, fallback=10, nopri=MAX (missing) → primary, fallback, nopri
        let pri = |e: &Endpoint| match e.url.as_str() {
            u if u.contains("primary") => 0,
            u if u.contains("fallback") => 10,
            _ => i64::MAX,
        };
        let ordered = order_endpoints(eps.clone(), &Ordering::Priority, pri);
        let urls: Vec<_> = ordered.iter().map(|e| e.url.as_str()).collect();
        assert_eq!(urls, ["http://primary:1", "http://fallback:1", "http://nopri:1"]);

        // Unordered = stable by url
        let un = order_endpoints(eps, &Ordering::Unordered, pri);
        let urls: Vec<_> = un.iter().map(|e| e.url.as_str()).collect();
        assert_eq!(urls, ["http://fallback:1", "http://nopri:1", "http://primary:1"]);
    }
}
