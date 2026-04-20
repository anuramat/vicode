use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;

use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

#[derive(Debug, Clone, Serialize)]
pub struct RecordedRequest {
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct MockResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

#[derive(Debug)]
pub struct MockServer {
    pub base_url: String,
    pub requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

pub async fn mock_server(responses: Vec<MockResponse>) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let shared_requests = Arc::clone(&requests);
    tokio::spawn(async move {
        for response in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            shared_requests.lock().unwrap().push(request);
            write_response(&mut socket, response).await;
        }
    });
    MockServer { base_url, requests }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> RecordedRequest {
    let mut buffer = Vec::new();
    let mut chunk = [0; 1024];
    let header_end = loop {
        let read = socket.read(&mut chunk).await.unwrap();
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let mut lines = headers.lines();
    let first = lines.next().unwrap();
    let mut first = first.split_whitespace();
    let _method = first.next().unwrap().to_string();
    let path = first.next().unwrap().to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<BTreeMap<_, _>>();
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    while buffer.len() < header_end + content_length {
        let read = socket.read(&mut chunk).await.unwrap();
        buffer.extend_from_slice(&chunk[..read]);
    }
    RecordedRequest {
        path,
        headers,
        body: String::from_utf8_lossy(&buffer[header_end..header_end + content_length])
            .into_owned(),
    }
}

async fn write_response(
    socket: &mut tokio::net::TcpStream,
    response: MockResponse,
) {
    socket
        .write_all(
            format!(
                "HTTP/1.1 {} OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.status,
                response.content_type,
                response.body.len(),
                response.body
            )
            .as_bytes(),
        )
        .await
        .unwrap();
}
