use std::{
    borrow::Borrow,
    collections::HashSet,
    convert::Infallible,
    convert::TryInto,
    env::args,
    error,
    ffi::OsStr,
    io::Error,
    net::{
        IpAddr::{V4, V6},
        Ipv4Addr,
        Ipv6Addr,
        SocketAddr,
    },
    path::Path,
    sync::Arc,
};

use futures::future;
use hyper::{
    Body,
    HeaderMap,
    http::HeaderValue,
    Method,
    Request,
    Response,
    server::Server,
    service::{
        make_service_fn,
        service_fn,
    },
    StatusCode,
};
use percent_encoding::percent_decode_str;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, SeekFrom},
};
use tokio_util::codec::{BytesCodec, FramedRead};

use crate::byte_range::{ByteRange, parse_range};
use crate::network::register_service;
use crate::scanner::{extract_served_files, RelativizedPath, scan_directory};

mod network;
mod scanner;
mod byte_range;

const PORT: u16 = 5000;

const PATH_MANIFEST: &str = "/";
const PATH_FILE_PREFIX: &str = "/file/";

const ALLOWED_ORIGIN: &str = "*";
const MAX_AGE: u32 = 48 * 60 * 60;

#[tokio::main]
async fn main() -> Result<(), Box<dyn error::Error>> {
    register_service(PORT)?;

    let folder = args().skip(1).next().unwrap();
    let catalogue = scan_directory(folder.as_ref(), folder.as_ref())?;
    let manifest = Arc::new(serde_json::to_string(&*catalogue).unwrap());
    let served_files = Arc::new(extract_served_files(&catalogue));

    let service = make_service_fn(move |_conn| {
        let manifest = manifest.clone();
        let served_files = served_files.clone();
        async {
            Ok::<_, Infallible>(service_fn(move |request: Request<Body>| {
                let manifest = manifest.clone();
                let served_files = served_files.clone();
                async move {
                    let mut response = Response::new(Body::empty());

                    match (request.method(), request.uri().path()) {
                        (&Method::GET, PATH_MANIFEST) => serve_manifest(manifest, &mut response),
                        (method @ &Method::GET, path) | (method @ &Method::OPTIONS, path) if path.starts_with(PATH_FILE_PREFIX) => {
                            response.headers_mut().insert("Accept-Ranges", HeaderValue::from_static("bytes"));
                            add_common_cors_headers(&mut response);

                            match method {
                                &Method::OPTIONS => {
                                    response.headers_mut().insert("Access-Control-Allow-Methods", HeaderValue::from_static("GET"));
                                }
                                &Method::GET => {
                                    serve_file(
                                        served_files,
                                        path.strip_prefix(PATH_FILE_PREFIX).unwrap(),
                                        request.headers(),
                                        &mut response,
                                    ).await;
                                }
                                _ => panic!("Unhandled method: {}", method)
                            }
                        }
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

async fn serve_file(served_files: Arc<HashSet<RelativizedPath>>, path: &str, headers: &HeaderMap<HeaderValue>, response: &mut Response<Body>) {
    let range_data = headers
        .get("Range")
        .map(|it| {
            it.to_str()
        });

    let range_data = match range_data {
        None => None,
        Some(Ok(data)) => Some(data),
        Some(Err(err)) => {
            eprintln!("Invalid range: {}", err);

            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Body::from("Invalid range");
            return;
        }
    };

    let requested_path = match percent_decode_str(path).decode_utf8() {
        Ok(path) => path,
        Err(_) => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Body::from("Path is not a valid file path");
            return;
        }
    };

    let relativized_path = served_files.iter()
        .find(|RelativizedPath { relative_path, .. }| {
            relative_path == <str as AsRef<OsStr>>::as_ref(requested_path.borrow())
        });

    let path = if let Some(RelativizedPath { path, .. }) = relativized_path {
        path
    } else {
        *response.status_mut() = StatusCode::NOT_FOUND;
        return;
    };

    let range = if let Some(range_data) = range_data {
        match parse_range::<()>(range_data) {
            Ok((_, mut ranges)) => {
                if ranges.len() == 1 {
                    Some(ranges.remove(0))
                } else {
                    None
                }
            }
            Err(_) => {
                eprintln!("Error while parsing the byte range: {}", range_data);

                *response.status_mut() = StatusCode::BAD_REQUEST;
                return;
            }
        }
    } else {
        None
    };

    if serve_file_range(path, &range, response).await.is_err() {
        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        *response.body_mut() = Body::from("Couldn't read the file");
    }
}

async fn serve_file_range(path: &Path, range: &Option<ByteRange>, response: &mut Response<Body>) -> Result<(), Error> {
    let file_len = std::fs::metadata(path)?.len();

    if let &Some(ref range) = range {
        let range_valid = match *range {
            ByteRange::StartingAt(start) => start < file_len,
            ByteRange::Last(len) => len <= file_len,
            ByteRange::FromToIncluding(start, end) => start < file_len && end < file_len && start <= end
        };

        let (status, served_range) = if range_valid {
            let (start, end) = match *range {
                ByteRange::StartingAt(start) => (start, file_len - 1),
                ByteRange::Last(len) => (file_len - len, file_len - 1),
                ByteRange::FromToIncluding(start, end) => (start, end)
            };
            (StatusCode::PARTIAL_CONTENT, format!("{}-{}", start, end))
        } else {
            (StatusCode::RANGE_NOT_SATISFIABLE, String::from("*"))
        };
        *response.status_mut() = status;
        response.headers_mut().insert("Content-Range", format!("bytes {}/{}", served_range, file_len).parse().unwrap());

        if !range_valid { return Ok(()); }
    }

    let mut file = File::open(path).await?;
    if let &Some(ref range) = range {
        match *range {
            ByteRange::StartingAt(start) => file.seek(SeekFrom::Start(start)),
            ByteRange::Last(end) => file.seek(SeekFrom::End(-(end as i64))),
            ByteRange::FromToIncluding(start, _) => file.seek(SeekFrom::Start(start))
        }.await?;
    }

    let body = if let Some(ByteRange::FromToIncluding(start, end)) = range {
        let file_part = file.take(end - start + 1);
        let reader = FramedRead::new(file_part, BytesCodec::new());
        Body::wrap_stream(reader)
    } else {
        let reader = FramedRead::new(file, BytesCodec::new());
        Body::wrap_stream(reader)
    };

    if let Some(mime) = mime_guess::from_path(path).first() {
        response.headers_mut().insert("Content-Type", mime.to_string().try_into().unwrap());
    }

    *response.body_mut() = body;
    Ok(())
}

fn add_common_cors_headers(response: &mut Response<Body>) {
    response.headers_mut().insert("Access-Control-Allow-Origin", HeaderValue::from_static(ALLOWED_ORIGIN));
    response.headers_mut().insert("Access-Control-Expose-Headers", HeaderValue::from_static("Content-Type, Accept-Encoding, Range"));
    response.headers_mut().insert("Access-Control-Max-Age", HeaderValue::from(MAX_AGE));
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c().await.unwrap();
}
