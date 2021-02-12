use std::{
    convert::Infallible,
    env::args,
    error,
    net::{
        IpAddr::{V4, V6},
        Ipv4Addr,
        Ipv6Addr,
        SocketAddr
    },
    sync::Arc,
};

use futures::future;
use hyper::{
    Body,
    Method,
    Request,
    Response,
    server::Server,
    service::{make_service_fn, service_fn},
    StatusCode,
};

use crate::network::register_service;
use crate::scanner::scan_directory;
use hyper::http::HeaderValue;

mod network;
mod scanner;

const PORT: u16 = 5000;

#[tokio::main]
async fn main() -> Result<(), Box<dyn error::Error>> {
    register_service(PORT)?;

    let folder = args().skip(1).next().unwrap();
    let catalogue = scan_directory(folder.as_ref(), folder.as_ref())?;
    let manifest = Arc::new(serde_json::to_string(&*catalogue).unwrap());

    let service = make_service_fn(move |_conn| {
        let manifest = manifest.clone();
        async {
            Ok::<_, Infallible>(service_fn(move |request: Request<Body>| {
                let manifest = manifest.clone();
                async move {
                    let mut response = Response::new(Body::empty());

                    match (request.method(), request.uri().path()) {
                        (&Method::GET, "/") => serve_manifest(manifest, &mut response),
                        _ => *response.status_mut() = StatusCode::NOT_FOUND
                    }

                    Ok::<_, Infallible>(response)
                }
            }))
        }
    });

    let handles: Vec<_> = vec![V4(Ipv4Addr::from(0)), V6(Ipv6Addr::from(0))]
        .into_iter()
        .map(|ip_addr| {
            let addr = SocketAddr::from((ip_addr, PORT));
            let server = Server::bind(&addr)
                .serve(service.clone())
                .with_graceful_shutdown(shutdown_signal());
            tokio::spawn(server)
        })
        .collect();

    if let (Err(e), ..) = future::select_all(handles).await {
        eprintln!("Server error: {}", e);
    }

    Ok(())
}

fn serve_manifest(manifest: Arc<String>, response: &mut Response<Body>) {
    response.headers_mut().insert("Content-Type", HeaderValue::from_static("application/json"));
    *response.body_mut() = Body::from(String::to_owned(&manifest))
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c().await.unwrap();
}
