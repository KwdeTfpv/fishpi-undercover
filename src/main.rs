use fishpi_undercover::*;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::registry()
        .with(EnvFilter::new("fishpi_undercover=info"))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // 初始化全局配置
    Config::init()?;
    let config = Config::get();
    tracing::info!("配置加载成功: {:?}", config);

    // 启动WebSocket服务器
    let server = WebSocketServer::new().await;
    let addr = config.server_addr().to_string();
    tracing::info!("WebSocket服务器启动在 {}", addr);
    server.start(&addr).await?;

    Ok(())
}
