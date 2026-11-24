//! Native API file transfer pacing, concurrency caps, space accounting, revocation, and slow-upload tests.
use super::*;

/// 768 KiB at 8 Mbit/s: the 256 KiB burst, then about half a second.
const PACED: usize = 768 * 1024;
const PACED_LEAST: Duration = Duration::from_millis(400);

#[tokio::test]
async fn a_download_is_paced_while_a_stream_runs_or_starts() {
    let files = Files::with(UPLOAD | DOWNLOAD);
    files.set("file_transfer_stream_mbps", "8");
    let bytes = files.put("game.sav", PACED);
    files.s.f.session(
        files.s.client.clone(),
        butterpollo_session::session::Role::Stream,
    );
    let started = std::time::Instant::now();
    assert_eq!(
        files.s.get("/files/content?name=game.sav").await.body,
        bytes
    );
    assert!(started.elapsed() >= PACED_LEAST, "{:?}", started.elapsed());

    // A stream starting paces too.
    let files = Files::with(DOWNLOAD);
    files.set("file_transfer_stream_mbps", "8");
    let bytes = files.put("game.sav", PACED);
    let launch = files.s.f.launch(
        files.s.client.clone(),
        butterpollo_session::session::Role::Stream,
    );
    files
        .s
        .host()
        .sessions
        .lock()
        .unwrap()
        .pending
        .insert("starting".into(), launch);
    let started = std::time::Instant::now();
    assert_eq!(
        files.s.get("/files/content?name=game.sav").await.body,
        bytes
    );
    assert!(started.elapsed() >= PACED_LEAST, "{:?}", started.elapsed());
}

#[tokio::test]
async fn an_upload_is_paced_while_a_stream_runs() {
    let files = Files::with(UPLOAD);
    files.set("file_transfer_stream_mbps", "8");
    files.s.f.session(
        files.s.client.clone(),
        butterpollo_session::session::Role::Stream,
    );
    let started = std::time::Instant::now();
    let reply = files.upload("paced.bin", vec![1; PACED]).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    assert!(started.elapsed() >= PACED_LEAST, "{:?}", started.elapsed());
}

#[tokio::test]
async fn downloads_are_capped_per_device_and_in_all() {
    let files = Files::with(DOWNLOAD);
    files.put("big.bin", 4 * MIB);
    // Made first: a new identity takes a while in a debug build.
    let others: Vec<_> = (0..3)
        .map(|n| device(&files.s.f, &format!("device-{n}"), DOWNLOAD))
        .collect();
    let fifth = device(&files.s.f, "device-fifth", DOWNLOAD);
    let me = files.s.connection.clone();
    let mut held = vec![];
    for _ in 0..2 {
        let response = files.start(&me, files.download("big.bin")).await;
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    files
        .s
        .get("/files/content?name=big.bin")
        .await
        .assert_error(409, "transfer_busy");
    for other in &others {
        for _ in 0..2 {
            let response = files.start(other, files.download("big.bin")).await;
            assert_eq!(response.status(), StatusCode::OK);
            held.push(response);
        }
    }
    assert_eq!(files.transfers().downloads(), 8);
    let refused = files.start(&fifth, files.download("big.bin")).await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);

    // A client that goes away gives its slot back.
    held.pop();
    until("a slot to free", || files.transfers().downloads() == 7).await;
    let response = files.start(&fifth, files.download("big.bin")).await;
    assert_eq!(response.status(), StatusCode::OK);
    drop((held, response));
    until("all slots to free", || files.transfers().downloads() == 0).await;
}

#[tokio::test]
async fn a_download_the_client_stops_taking_ends_and_closes_its_file() {
    let files = Files::with(DOWNLOAD);
    files
        .transfers()
        .limit(|limits| limits.idle = Duration::from_millis(300));
    files.put("big.bin", 4 * MIB);
    let me = files.s.connection.clone();
    let response = files.start(&me, files.download("big.bin")).await;
    assert_eq!(response.status(), StatusCode::OK);
    until("the stalled download to end", || {
        files.transfers().downloads() == 0
    })
    .await;
    std::fs::remove_file(files.dir.join("big.bin")).unwrap();
    let body = to_bytes(response.into_body(), usize::MAX).await;
    assert!(body.is_err(), "the body ends short");
}

#[tokio::test]
async fn a_file_another_program_holds_is_busy() {
    use std::os::windows::fs::OpenOptionsExt;
    let files = Files::with(DOWNLOAD);
    files.put("locked.txt", 10);
    let locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(files.dir.join("locked.txt"))
        .unwrap();
    files
        .s
        .get("/files/content?name=locked.txt")
        .await
        .assert_error(409, "transfer_busy");
    drop(locked);
    assert_eq!(
        files.s.get("/files/content?name=locked.txt").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_device_refused_for_room_can_send_again_at_once() {
    const GIB: u64 = 1 << 30;
    let files = Files::with(UPLOAD | DOWNLOAD);
    files.set("file_transfer_max_mb", "16384");
    // 8 GiB beside the 2 GiB (2 %) reserve.
    files.transfers().fake_space(10 * GIB, 100 * GIB);
    // The refused upload's writer is still running when the retry comes.
    files.transfers().hold_refusals(Duration::from_secs(2));
    let refused = files
        .upload_as("big.bin", Some(9 * GIB), b"x".to_vec())
        .await;
    assert_eq!(refused.status, StatusCode::INSUFFICIENT_STORAGE);
    let sent = files.upload("small.txt", b"hi".to_vec()).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.text());
}

#[tokio::test]
async fn an_upload_leaves_room_for_the_uploads_running() {
    const GIB: u64 = 1 << 30;
    let files = Files::with(UPLOAD | DOWNLOAD);
    files.set("file_transfer_max_mb", "8192");
    let other = device(&files.s.f, "second-device", UPLOAD);
    // 8 GiB beside the 2 GiB (2 %) reserve.
    files.transfers().fake_space(10 * GIB, 100 * GIB);
    assert_eq!(files.free_bytes().await, 8 * GIB);
    let (first, mut writer) = files.streamed("first.bin", 5 * GIB);
    let first = tokio::spawn({
        let router =
            router(files.s.host().clone(), true).layer(Extension(files.s.connection.clone()));
        async move { router.oneshot(first).await.unwrap().status() }
    });
    writer.write_all(b"half").await.unwrap();
    until("the first upload to start", || {
        files.names() == ["first.bin.part"]
    })
    .await;
    let mut free = 0;
    for _ in 0..500 {
        free = files.free_bytes().await;
        if free == 3 * GIB + 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(free, 3 * GIB + 4, "less what the first upload still writes");

    let second = |length: u64| {
        files.request(
            "POST",
            "/files?name=second.bin",
            Some(length),
            Body::from("x"),
        )
    };
    let refused = files.start(&other, second(4 * GIB)).await;
    assert_eq!(refused.status(), StatusCode::INSUFFICIENT_STORAGE);
    // This one fits, and only falls short of its length.
    let short = files.start(&other, second(3 * GIB)).await;
    assert_eq!(short.status(), StatusCode::BAD_REQUEST);

    drop(writer);
    assert_eq!(first.await.unwrap(), StatusCode::BAD_REQUEST);
    assert!(files.names().is_empty(), "{:?}", files.names());
    assert_eq!(files.free_bytes().await, 8 * GIB);
}

#[tokio::test]
async fn an_upload_slower_than_the_least_rate_is_refused() {
    let files = Files::with(UPLOAD);
    files
        .transfers()
        .limit(|limits| limits.grace = Duration::from_millis(300));
    let (request, mut writer) = files.streamed("slow.bin", MIB as u64);
    writer.write_all(b"x").await.unwrap();
    let started = std::time::Instant::now();
    let reply = files.send(request).await;
    reply.assert_error(400, "incomplete_upload");
    assert!(reply.text().contains("slower"), "{}", reply.text());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(files.names().is_empty(), "{:?}", files.names());
    drop(writer);
}

#[tokio::test]
async fn turning_file_transfer_off_ends_an_upload_and_deletes_its_part() {
    let files = Files::with(UPLOAD);
    let (request, mut writer) = files.streamed("cut.bin", MIB as u64);
    let upload = tokio::spawn({
        let router =
            router(files.s.host().clone(), true).layer(Extension(files.s.connection.clone()));
        async move { router.oneshot(request).await.unwrap().status() }
    });
    writer.write_all(b"half").await.unwrap();
    until("the upload to start", || files.names() == ["cut.bin.part"]).await;
    files.set("file_transfer", "disabled");
    let status = tokio::time::timeout(Duration::from_secs(5), upload)
        .await
        .expect("the upload ends")
        .unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
    until("the part file to go", || files.names().is_empty()).await;
    drop(writer);
}

#[tokio::test]
async fn revoking_download_files_ends_a_download() {
    let files = Files::with(DOWNLOAD);
    files.put("big.bin", 4 * MIB);
    let me = files.s.connection.clone();
    let response = files.start(&me, files.download("big.bin")).await;
    assert_eq!(response.status(), StatusCode::OK);
    files.s.host().paired.write().unwrap().clients[0].perm = UPLOAD;
    until("the download to end", || files.transfers().downloads() == 0).await;
    let body = to_bytes(response.into_body(), usize::MAX).await;
    assert!(body.is_err(), "the body ends short");
}

/// Serves one connection of the router on a real socket. Without
/// `connection` the caller is an unpaired plain-HTTP client.
async fn serve_once(files: &Files, connection: Option<Connection>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut router = router(files.s.host().clone(), true);
    if let Some(connection) = connection {
        // Inside the plain connection's layer, so it wins.
        router = router.layer(Extension(connection));
    }
    tokio::spawn(async move {
        let (socket, peer) = listener.accept().await.unwrap();
        butterpollo_stream::tls::serve_connection(socket, peer, address, router, None).await;
    });
    address
}
fn upload_head(length: u64) -> String {
    format!(
        "POST {PREFIX}/files?name=a.bin HTTP/1.1\r\nHost: pc\r\nContent-Length: {length}\r\n\r\n"
    )
}
/// Writes zeros until the host stops reading; how much went out.
async fn blast(mut writer: tokio::net::tcp::OwnedWriteHalf) -> u64 {
    let chunk = vec![0; 64 * 1024];
    let mut sent = 0u64;
    while writer.write_all(&chunk).await.is_ok() {
        sent += chunk.len() as u64;
    }
    sent
}

#[tokio::test]
async fn an_unpaired_upload_is_answered_and_closed_at_once() {
    let files = Files::with(UPLOAD);
    let address = serve_once(&files, None).await;
    let socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let (_reader, mut writer) = socket.into_split();
    writer
        .write_all(upload_head(1 << 40).as_bytes())
        .await
        .unwrap();
    let sent = tokio::time::timeout(Duration::from_secs(5), blast(writer))
        .await
        .expect("the host closes an unpaired upload at once");
    assert!(sent < 64 * MIB as u64, "{sent}");
    assert!(files.names().is_empty());
}

#[tokio::test]
async fn a_refused_upload_is_drained_for_a_while_then_closed() {
    let files = Files::with(UPLOAD);
    files.set("file_transfer_max_mb", "1");
    files
        .transfers()
        .limit(|limits| limits.drain_time = Duration::from_secs(1));
    let address = serve_once(&files, Some(files.s.connection.clone())).await;
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let started = std::time::Instant::now();
    socket
        .write_all(upload_head(64 * MIB as u64).as_bytes())
        .await
        .unwrap();
    socket.write_all(&vec![0; MIB]).await.unwrap();
    // The client reads its answer, not a reset, and the host closes.
    let mut answer = vec![];
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio::io::AsyncReadExt::read_to_end(&mut socket, &mut answer),
    )
    .await
    .expect("the host closes after the drain")
    .unwrap();
    let answer = String::from_utf8_lossy(&answer).to_lowercase();
    assert!(answer.starts_with("http/1.1 413"), "{answer}");
    assert!(answer.contains("connection: close"), "{answer}");
    assert!(answer.contains("too_large"), "{answer}");
    assert!(started.elapsed() >= Duration::from_millis(900));

    // And no more than its bytes, however fast the client sends.
    let files = Files::with(UPLOAD);
    files.set("file_transfer_max_mb", "1");
    files.transfers().limit(|limits| {
        limits.drain_bytes = 2 * MIB as u64;
        limits.drain_time = Duration::from_secs(60);
    });
    let address = serve_once(&files, Some(files.s.connection.clone())).await;
    let socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let (_reader, mut writer) = socket.into_split();
    writer
        .write_all(upload_head(1 << 40).as_bytes())
        .await
        .unwrap();
    let sent = tokio::time::timeout(Duration::from_secs(10), blast(writer))
        .await
        .expect("the host stops draining after its bytes");
    assert!(sent < 64 * MIB as u64, "{sent}");
}
