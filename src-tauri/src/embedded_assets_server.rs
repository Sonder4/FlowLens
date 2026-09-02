//! Serves the frontend assets embedded in the executable over a private
//! loopback port. This avoids relying on WebView2's tauri.localhost request
//! interception for the first document navigation.

#[cfg(not(debug_assertions))]
use std::io::{self, Read, Write};
#[cfg(not(debug_assertions))]
use std::net::{Ipv4Addr, TcpListener, TcpStream};
#[cfg(not(debug_assertions))]
use std::time::Duration;

#[cfg(not(debug_assertions))]
use tauri::{AppHandle, AssetResolver};

#[cfg(not(debug_assertions))]
pub struct EmbeddedAssetsServer {
    port: u16,
}

#[cfg(not(debug_assertions))]
impl EmbeddedAssetsServer {
    pub fn start(app: &AppHandle) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let assets = app.asset_resolver();

        std::thread::Builder::new()
            .name("flowlens_assets".to_string())
            .spawn(move || serve(listener, assets))?;

        eprintln!("[flowlens] embedded asset server listening on 127.0.0.1:{port}");
        Ok(Self { port })
    }

    pub fn url_for(&self, asset_path: &str) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, asset_path)
    }

    pub fn url_pattern(&self) -> String {
        format!("http://127.0.0.1:{}/*", self.port)
    }
}

#[cfg(not(debug_assertions))]
fn serve(listener: TcpListener, assets: AssetResolver<tauri::Wry>) {
    for connection in listener.incoming() {
        match connection {
            Ok(stream) => serve_connection(stream, &assets),
            Err(error) => eprintln!("[flowlens] embedded asset server accept failed: {error}"),
        }
    }
}

#[cfg(not(debug_assertions))]
fn serve_connection(mut stream: TcpStream, assets: &AssetResolver<tauri::Wry>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut request = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 2048];

    while request.len() < 8192 {
        let Ok(read) = stream.read(&mut chunk) else {
            return;
        };
        if read == 0 {
            return;
        }
        request.extend_from_slice(&chunk[..read]);
        if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            break;
        }
    }

    let request = String::from_utf8_lossy(&request);
    let Some((method, raw_path)) = request.lines().next().and_then(|line| {
        let mut parts = line.split_ascii_whitespace();
        Some((parts.next()?, parts.next()?))
    }) else {
        return;
    };

    if method != "GET" && method != "HEAD" {
        write_response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain",
            b"",
            false,
        );
        return;
    }

    let Some(asset_path) = asset_path(raw_path) else {
        write_response(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            b"",
            method == "GET",
        );
        return;
    };

    match assets.get(asset_path) {
        Some(asset) => write_response(
            &mut stream,
            "200 OK",
            &asset.mime_type,
            &asset.bytes,
            method == "GET",
        ),
        None => write_response(
            &mut stream,
            "404 Not Found",
            "text/plain",
            b"Not found",
            method == "GET",
        ),
    }
}

#[cfg(any(not(debug_assertions), test))]
fn asset_path(raw_path: &str) -> Option<String> {
    let path = raw_path.split('?').next()?.trim_start_matches('/');
    let path = if path.is_empty() {
        "dashboard.html"
    } else {
        path
    };

    if path.contains('\\')
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return None;
    }

    Some(path.to_string())
}

#[cfg(not(debug_assertions))]
fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    include_body: bool,
) {
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(headers.as_bytes());
    if include_body {
        let _ = stream.write_all(body);
    }
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::asset_path;
    use std::str::FromStr;
    use tauri::utils::acl::RemoteUrlPattern;

    #[test]
    fn maps_root_and_strips_query() {
        assert_eq!(asset_path("/").as_deref(), Some("dashboard.html"));
        assert_eq!(
            asset_path("/assets/app.js?v=1").as_deref(),
            Some("assets/app.js")
        );
    }

    #[test]
    fn rejects_path_traversal() {
        assert_eq!(asset_path("/../secret"), None);
        assert_eq!(asset_path("/assets\\..\\secret"), None);
        assert_eq!(asset_path("/assets//app.js"), None);
    }

    #[test]
    fn matches_only_its_loopback_port() {
        let pattern = RemoteUrlPattern::from_str("http://127.0.0.1:52923/*").unwrap();
        let url = "http://127.0.0.1:52923/dashboard.html".parse().unwrap();
        let other_port = "http://127.0.0.1:52924/dashboard.html".parse().unwrap();

        assert!(pattern.test(&url));
        assert!(!pattern.test(&other_port));
    }
}
