use fishpi_undercover::*;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化全局配置
    Config::init()?;
    let config = Config::get();
    
    // 初始化日志
    tracing_subscriber::registry()
        .with(EnvFilter::new(&config.log_filter()))
        .with(tracing_subscriber::fmt::layer())
        .init();
        
    tracing::info!("配置加载成功: {:?}", config);

    let server = WebSocketServer::new().await;
    
    
    // 获取HTTP和WebSocket地址
    let http_addr = config.http_addr().to_string();
    let ws_addr = config.ws_addr().to_string();
    
    tracing::info!("HTTP服务器启动在 {}", http_addr);
    tracing::info!("WebSocket服务器启动在 {}", ws_addr);
    
    server.start_http_server(&http_addr).await?;
    
    server.start_ws_server(&ws_addr).await?;

    Ok(())
}
