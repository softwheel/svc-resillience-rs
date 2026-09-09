//! Run with `cargo run --example envoy_policy --features envoy`.
#[cfg(feature = "envoy")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use softwheel_resilience::envoy::{
        ClusterLimits, Priority, Resource, RetryOutcome, RoutePolicy,
    };
    let route = RoutePolicy::from_json(include_str!("envoy/route.json"))?;
    // Keep one limits handle per actual cluster; clones share its accounting.
    let limits = ClusterLimits::from_json(include_str!("envoy/circuit-breakers.json"))?;
    let plan = route.plan();
    let _request = limits.try_acquire(Priority::Default, Resource::Request)?;
    println!(
        "primary={}, mirrors={}, timeout={:?}",
        plan.cluster(),
        plan.mirrors().len(),
        route.timeout()
    );
    println!(
        "first retry delay after 503: {:?}",
        route.retry_delay(1, RetryOutcome::HttpStatus(503))
    );
    // This example only plans traffic. The transport enforces deadlines, holds
    // permits for actual resource lifetimes, and dispatches isolated mirrors.
    Ok(())
}
#[cfg(not(feature = "envoy"))]
fn main() {
    eprintln!("enable --features envoy");
}
