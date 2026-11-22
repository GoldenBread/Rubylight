//! Test-harness loopback webhook receiver.
use std::collections::HashMap;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};

pub struct Received {
    /// Header names in lower case.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// A receiver answering its requests with `statuses` in turn (the last
/// repeats); 0 never answers. The URL has a path and a query.
pub async fn receiver(statuses: Vec<u16>) -> (String, mpsc::UnboundedReceiver<Received>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/hook?token=private",
        listener.local_addr().unwrap()
    );
    let (sender, received) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut answered = 0;
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let status = statuses[answered.min(statuses.len() - 1)];
            answered += 1;
            let sender = sender.clone();
            tokio::spawn(async move {
                let mut data = Vec::new();
                let mut buffer = [0; 4096];
                let (headers, length) = loop {
                    let read = stream.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        return;
                    }
                    data.extend_from_slice(&buffer[..read]);
                    if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8(data[..end].to_vec()).unwrap();
                        let headers = head
                            .lines()
                            .skip(1)
                            .filter_map(|line| line.split_once(':'))
                            .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_owned()))
                            .collect::<HashMap<_, _>>();
                        data.drain(..end + 4);
                        let length = headers["content-length"].parse::<usize>().unwrap();
                        break (headers, length);
                    }
                };
                while data.len() < length {
                    let read = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(read, 0, "the body ended early");
                    data.extend_from_slice(&buffer[..read]);
                }
                let _ = sender.send(Received {
                    headers,
                    body: data,
                });
                if status == 0 {
                    std::future::pending::<()>().await;
                }
                let answer = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                );
                stream.write_all(answer.as_bytes()).await.unwrap();
            });
        }
    });
    (url, received)
}
