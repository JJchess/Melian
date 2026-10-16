use guard_core::controller::{Controller, CountryProbe, HttpCountryProbe, Mihomo};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let s = Mihomo::new("verge-mihomo")?.read().await?;
    println!(
        "mode={} port={} node={} path={:?}",
        s.mode, s.port, s.node, s.path
    );
    println!("country={}", HttpCountryProbe.check(s.port).await?);
    Ok(())
}
