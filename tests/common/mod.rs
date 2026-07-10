use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

/// One-shot mock HTTP server. Serves one connection with `status` + `body`
/// (using `content_type`), then the thread exits. Returns "http://127.0.0.1:PORT".
/// Suitable for the small request bodies these tests send.
pub fn mock(status: u16, content_type: &str, body: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let reason = if status == 200 { "OK" } else { "ERROR" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // Drain the request so the client's write side doesn't error.
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{addr}")
}
