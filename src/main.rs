use anyhow::Result;
use rclone_browser_ng::{
    api,
    runner::{Config, Service},
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rclone_browser_ng=info".into()),
        )
        .init();
    let service = Service::new(Config::from_env()?)?;
    let listener = tokio::net::TcpListener::bind(&service.config.bind_addr).await?;
    tracing::info!(address = %listener.local_addr()?, "Rust rclone service listening");
    let handles = service.start();
    let shutdown = service.shutdown.clone();
    let result = axum::serve(listener, api::router(service.clone()))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            shutdown.cancel();
        })
        .await;
    service.shutdown.cancel();
    for handle in handles {
        handle.await?;
    }
    service
        .store
        .interrupt_active("Service stopped before this queued run started")?;
    result?;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
