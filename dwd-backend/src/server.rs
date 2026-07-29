use actix_web::{get, web, App, HttpResponse, HttpServer};
use bytes::Bytes;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};

/// Content type of every response. Actix and h3 use different `http` crate
/// versions, so they cannot share the header value itself.
const CONTENT_TYPE_TEXT: &str = "text/plain; charset=utf-8";
const CONTENT_PREFIX: &str = "Content: ";

struct AppState {
    body: Bytes,
}

/// Builds the full response body once at startup.
pub fn build_body(content: String) -> Bytes {
    let mut body = String::with_capacity(CONTENT_PREFIX.len() + content.len());
    body.push_str(CONTENT_PREFIX);
    body.push_str(&content);
    Bytes::from(body)
}

#[get("/")]
async fn index(data: web::Data<AppState>) -> HttpResponse {
    HttpResponse::Ok()
        .insert_header((
            actix_web::http::header::CONTENT_TYPE,
            actix_web::http::header::HeaderValue::from_static(CONTENT_TYPE_TEXT),
        ))
        .body(data.body.clone())
}

/// Resolves an address and port for the QUIC listener.
pub fn resolve_socket_addr(address: &str, port: u16) -> io::Result<SocketAddr> {
    (address, port).to_socket_addrs()?.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("failed to resolve address {address}:{port}"),
        )
    })
}

/// Binds the QUIC endpoint before server tasks are spawned, so startup reports
/// invalid addresses and port conflicts immediately.
pub fn bind_http3(addr: SocketAddr, server_config: quinn::ServerConfig) -> io::Result<quinn::Endpoint> {
    quinn::Endpoint::server(server_config, addr)
}

/// Runs the QUIC/HTTP3 server. Each incoming connection is handled in its own
/// task, and every request is answered with the same content.
pub async fn run_http3_server(endpoint: quinn::Endpoint, body: Bytes) {
    while let Some(incoming) = endpoint.accept().await {
        let body = body.clone();
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(conn) => conn,
                Err(error) => {
                    eprintln!("HTTP3: failed to establish connection: {error}");
                    return;
                }
            };

            if let Err(error) = handle_h3_connection(conn, body).await {
                eprintln!("HTTP3: connection error: {error}");
            }
        });
    }
}

async fn handle_h3_connection(
    conn: quinn::Connection,
    body: Bytes,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut h3_conn = h3::server::Connection::new(h3_quinn::Connection::new(conn)).await?;

    loop {
        match h3_conn.accept().await {
            Ok(Some(resolver)) => {
                let body = body.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_h3_request(resolver, body).await {
                        eprintln!("HTTP3: request error: {error}");
                    }
                });
            }
            Ok(None) => break,
            Err(error) => return Err(Box::new(error)),
        }
    }

    Ok(())
}

async fn handle_h3_request<C>(
    resolver: h3::server::RequestResolver<C, Bytes>,
    body: Bytes,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    C: h3::quic::Connection<Bytes>,
{
    let (_request, mut stream) = resolver.resolve_request().await?;
    let response = http::Response::builder()
        .status(http::StatusCode::OK)
        .header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static(CONTENT_TYPE_TEXT),
        )
        .body(())?;

    stream.send_response(response).await?;
    stream.send_data(body).await?;
    stream.finish().await?;

    Ok(())
}

/// Runs HTTP and optional HTTPS listeners in one Actix server.
pub async fn run_tcp_servers(
    address: &str,
    http_port: Option<u16>,
    https: Option<(u16, rustls::ServerConfig)>,
    body: Bytes,
) -> io::Result<()> {
    let state = web::Data::new(AppState { body });
    let mut server = HttpServer::new(move || App::new().app_data(state.clone()).service(index));

    if let Some(http_port) = http_port {
        server = server.bind((address, http_port))?;
        println!("HTTP listening on {address}:{http_port}");
    }

    if let Some((https_port, tls_config)) = https {
        server = server.bind_rustls_0_23((address, https_port), tls_config)?;
        println!("HTTPS listening on {address}:{https_port}");
    }

    server.run().await
}
