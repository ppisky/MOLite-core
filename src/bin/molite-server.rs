use molite_core::{http, runtime::Engine};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::var("MOLITE_DATA").unwrap_or_else(|_| "data".into());
    let address: std::net::SocketAddr = std::env::var("MOLITE_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:3791".into())
        .parse()?;
    let token = std::env::var("MOLITE_HTTP_TOKEN")
        .ok()
        .filter(|v| !v.is_empty());
    if !address.ip().is_loopback() && token.is_none() {
        return Err("non-loopback listeners require MOLITE_HTTP_TOKEN".into());
    }
    let engine = Engine::open(root)?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    eprintln!("MOLite listening on {}", listener.local_addr()?);
    axum::serve(listener, http::router(engine, token))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
