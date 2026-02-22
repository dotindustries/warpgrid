//! HTTP trigger handler.
//!
//! `HttpTrigger` manages a hyper HTTP server that forwards requests
//! to Wasm components via the wasi-http proxy interface.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tracing::{error, info};

/// Callback type for handling HTTP requests.
///
/// The router provides this callback to the trigger — it maps requests
/// to the appropriate Wasm component and returns responses.
pub type RequestHandler =
    Arc<dyn Fn(Request<Incoming>) -> BoxFuture + Send + Sync>;

type BoxFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = anyhow::Result<Response<Full<Bytes>>>> + Send>,
>;

/// HTTP trigger server.
///
/// Binds to a TCP port and forwards incoming HTTP requests to a
/// handler callback. The handler is responsible for routing requests
/// to the appropriate Wasm component.
pub struct HttpTrigger {
    bind_addr: SocketAddr,
    handler: RequestHandler,
}

impl HttpTrigger {
    /// Create a new HTTP trigger bound to the given address.
    pub fn new(bind_addr: SocketAddr, handler: RequestHandler) -> Self {
        Self { bind_addr, handler }
    }

    /// Start the HTTP server.
    ///
    /// This runs until the shutdown signal is received. Spawns a
    /// tokio task per connection using HTTP/1.1.
    pub async fn serve(self, mut shutdown: tokio::sync::watch::Receiver<bool>) -> anyhow::Result<()> {
        let listener = TcpListener::bind(self.bind_addr)
            .await
            .context("failed to bind HTTP trigger")?;

        info!(addr = %self.bind_addr, "HTTP trigger listening");

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    let (stream, peer_addr) = accept_result.context("accept failed")?;
                    let handler = self.handler.clone();

                    tokio::spawn(async move {
                        let io = TokioIo::new(stream);
                        let svc = service_fn(move |req: Request<Incoming>| {
                            let handler = handler.clone();
                            async move {
                                match handler(req).await {
                                    Ok(resp) => Ok::<_, hyper::Error>(resp),
                                    Err(e) => {
                                        error!(%peer_addr, error = %e, "request handler failed");
                                        Ok(Response::builder()
                                            .status(500)
                                            .body(Full::new(Bytes::from("Internal Server Error")))
                                            .unwrap())
                                    }
                                }
                            }
                        });

                        if let Err(e) = http1::Builder::new()
                            .serve_connection(io, svc)
                            .await
                        {
                            error!(%peer_addr, error = %e, "connection error");
                        }
                    });
                }
                _ = shutdown.changed() => {
                    info!("HTTP trigger shutting down");
                    break;
                }
            }
        }

        Ok(())
    }
}

/// Create a simple echo handler for testing.
///
/// Returns the request path and method as the response body.
pub fn echo_handler() -> RequestHandler {
    Arc::new(|req: Request<Incoming>| {
        Box::pin(async move {
            let body = format!(
                "{} {}",
                req.method(),
                req.uri().path()
            );
            Ok(Response::builder()
                .status(200)
                .header("content-type", "text/plain")
                .body(Full::new(Bytes::from(body)))
                .unwrap())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_trigger_creation() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let handler = echo_handler();
        let trigger = HttpTrigger::new(addr, handler);
        assert_eq!(trigger.bind_addr, addr);
    }

    #[tokio::test]
    async fn http_trigger_serves_and_shuts_down() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let handler = echo_handler();
        let trigger = HttpTrigger::new(addr, handler);

        let (tx, rx) = tokio::sync::watch::channel(false);

        let server = tokio::spawn(async move {
            trigger.serve(rx).await
        });

        // Give it a moment to bind.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Signal shutdown.
        tx.send(true).unwrap();

        let result = server.await.unwrap();
        assert!(result.is_ok());
    }
}
