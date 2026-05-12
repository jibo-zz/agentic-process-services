use cli::client::Fetcher;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fetcher = Fetcher::local();
    let health = fetcher.check_health().await?;

    println!("{}: {}", health.service, health.status);

    Ok(())
}
