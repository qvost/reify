//! `cargo run --release -p reify-server --example profile_mount`
use reify_core::domain::Target;
use reify_probe::ProbeEngine;
use reify_server::{BrowserAppBuilder, ReifyServer, warm_profiles};
use reify_synth::SynthEngine;
use std::{sync::Arc, time::Instant};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let startup = Instant::now();
    warm_profiles()?;
    println!(
        "one-time catalog initialization: {:.3}ms",
        startup.elapsed().as_secs_f64() * 1000.0
    );
    let builder = Arc::new(BrowserAppBuilder::new(
        Arc::new(ProbeEngine::with_chromium(
            "/missing/profile-benchmark-chromium",
        )),
        Arc::new(SynthEngine::new()),
    ));
    let target = Target::new(
        "https://news.ycombinator.com",
        Some("search stories".into()),
    )?;
    let mut samples = Vec::with_capacity(1000);
    for _ in 0..1000 {
        // Every sample starts with a separate empty cache and mount registry.
        let server = ReifyServer::with_builder(builder.clone());
        let target = target.clone();
        let start = Instant::now();
        let mounted = server.mount(target).await?;
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        assert!(!mounted.cache_hit);
        assert_eq!(mounted.app.tools[0].definition.name, "search_stories");
        let prepared = server.store().prepared(&mounted.key).await?;
        assert!(prepared.is_some());
        samples.push(elapsed);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "1000 empty-cache profile mounts: p50={:.3}ms p95={:.3}ms max={:.3}ms",
        samples[500], samples[950], samples[999]
    );
    Ok(())
}
