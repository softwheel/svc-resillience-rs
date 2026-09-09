//! Export through the public API, for validation by an actual Envoy binary.
#[cfg(feature = "envoy")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use softwheel_resilience::envoy::{ClusterLimits, RoutePolicy};
    let route = RoutePolicy::from_json(include_str!("envoy/route.json"))?;
    let limits = ClusterLimits::from_json(include_str!("envoy/circuit-breakers.json"))?;
    println!(
        "{{\"route\":{},\"circuit_breakers\":{}}}",
        route.to_json()?,
        limits.to_json()?
    );
    Ok(())
}
#[cfg(not(feature = "envoy"))]
fn main() {
    eprintln!("enable --features envoy");
    std::process::exit(1);
}
