use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        WebSocketUpgrade,
    },
    response::IntoResponse,
    Extension, Json,
};
use axum_extra::TypedHeader;
use log::{error, info, warn};
use tap::TapFallible;
use tokio::sync::broadcast;

use crate::{config::Config, database::BroadcastEvent, types::Auth};

use super::types::RealIP;

pub async fn route(
    config: Config,
    broadcast: broadcast::Receiver<BroadcastEvent>,
) -> anyhow::Result<()> {
    let inner_broadcast = Arc::new(broadcast.resubscribe());
    let password = Arc::new(config.web().access_key().to_string());

    let router = axum::Router::new()
        .route("/ws", axum::routing::get(handle_upgrade))
        .route(
            "/",
            axum::routing::get(|| async {
                Json(serde_json::json!({"version": env!("CARGO_PKG_VERSION")}))
            }),
        )
        .layer(Extension(inner_broadcast))
        .layer(Extension(password));

    let listener = tokio::net::TcpListener::bind(config.web().bind()).await?;

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let mut recv = broadcast.resubscribe();
            while let Ok(BroadcastEvent::Exit) = recv.recv().await {}
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        })
        .await?;
    Ok(())
}

pub async fn handle_upgrade(
    ws: WebSocketUpgrade,
    TypedHeader(real_ip): TypedHeader<RealIP>,
    Extension(broadcast): Extension<Arc<broadcast::Sender<BroadcastEvent>>>,
    Extension(password): Extension<Arc<String>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| async move {
        let ip = real_ip.into_inner();
        info!("Accept request from {:?}", &ip);
        handle_code_query(socket, broadcast.subscribe(), password, &ip)
            .await
            .tap_err(|e| error!("Handle {} websocket error: {:?}", ip, e))
            .ok();
    })
}

pub async fn handle_code_query(
    mut socket: WebSocket,
    mut broadcast: broadcast::Receiver<BroadcastEvent>,
    password: Arc<String>,
    ip: &str,
) -> anyhow::Result<()> {
    let mut is_register = false;

    loop {
        tokio::select! {
            Ok(event) = broadcast.recv() => {
                if !is_register {
                    continue;
                }
                match event {
                    BroadcastEvent::NewCode(code) => {
                        socket.send(Message::Text(code)).await?;
                    }
                    BroadcastEvent::Exit => {
                        socket.send(Message::Text("close".to_string())).await.ok();
                        break;
                    }
                }
            }
            Some(message) = socket.recv() => {
                if let Ok(message) = message {
                    if let Ok(text) = message.to_text() {
                        if text.eq("close") {
                            break;
                        }
                        if let Ok(header) = Auth::try_from(text) {
                            if header.check(&password) {
                                is_register = true;
                            } else {
                                warn!("ID: {} password check failed", header.codename());
                            }
                        }
                    } else {
                        warn!("Skip unreadable bytes: {:?}", message);
                    }
                } else {
                    return Ok(());
                }
            }
        }
    }
    socket.close().await.ok();
    info!("Disconnect from: {}", ip);
    Ok(())
}
