"""Embed Rust-exported policy fragments in a minimal real Envoy v3 bootstrap.

Usage: cargo run --quiet --example export_envoy --features envoy | \
python scripts/envoy_bootstrap.py > /tmp/envoy-bootstrap.json
Validation does not start a proxy or contact an upstream service.
"""
import json
import sys

fragments = json.load(sys.stdin)
route = fragments["route"]
names = {c["name"] for c in route.get("weighted_clusters", {}).get("clusters", [])}
if "cluster" in route:
    names.add(route["cluster"])
names.update(m["cluster"] for m in route.get("request_mirror_policies", []))

def address(port):
    return {"socket_address": {"address": "127.0.0.1", "port_value": port}}

clusters = [{
    "name": name, "type": "STATIC", "connect_timeout": "1s",
    "circuit_breakers": fragments["circuit_breakers"],
    "load_assignment": {"cluster_name": name, "endpoints": [{"lb_endpoints": [
        {"endpoint": {"address": address(18080)}}
    ]}]}
} for name in sorted(names)]
bootstrap = {"static_resources": {
    "clusters": clusters,
    "listeners": [{"name": "policy-validation", "address": address(10000),
        "filter_chains": [{"filters": [{
            "name": "envoy.filters.network.http_connection_manager",
            "typed_config": {
                "@type": "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager",
                "stat_prefix": "policy",
                "route_config": {"name": "policy", "virtual_hosts": [{
                    "name": "policy", "domains": ["*"], "routes": [{"match": {"prefix": "/"}, "route": route}]
                }]},
                "http_filters": [{"name": "envoy.filters.http.router", "typed_config": {
                    "@type": "type.googleapis.com/envoy.extensions.filters.http.router.v3.Router"
                }}]
            }
        }]}]
    }]
}}
json.dump(bootstrap, sys.stdout, indent=2)
print()
