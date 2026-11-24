//! File transfer (`docs/native-api.md`, "Files") through the router, into a
//! temporary transfer folder. The host runs as the developer here, so the
//! folder is opened without impersonation.
use super::*;
use axum::response::Response;
use rubylight_native_api::{
    feature,
    files::{FileList, Uploaded},
};
use std::{path::PathBuf, time::Duration};
use tokio::io::AsyncWriteExt;

const UPLOAD: u32 = 1 << 18;
const DOWNLOAD: u32 = 1 << 19;
const MIB: usize = 1024 * 1024;

/// A paired device and a host with File transfer on, into its own folder.
struct Files {
    s: Setup,
    dir: PathBuf,
}
impl Files {
    fn with(perm: u32) -> Self {
        let s = Setup::with(perm);
        let dir = std::env::temp_dir().join(format!("butterpollo-files-{}", uuid::Uuid::new_v4()));
        let files = Self { s, dir };
        files.set("file_transfer", "enabled");
        files.set("file_transfer_dir", &files.dir.display().to_string());
        // Room for every test, however full the disk of the PC running it.
        files.transfers().fake_space(1 << 40, 1 << 41);
        files
    }
    fn set(&self, key: &str, value: &str) {
        self.s
            .host()
            .config
            .write()
            .unwrap()
            .values
            .insert(key.into(), value.into());
    }
    /// The names in the folder, `.part` files included.
    fn names(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return vec![];
        };
        let mut names: Vec<String> = entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
    fn request(&self, method: &str, path: &str, length: Option<u64>, body: Body) -> Request<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("{PREFIX}{path}"));
        if let Some(length) = length {
            request = request.header(header::CONTENT_LENGTH, length);
        }
        request.body(body).unwrap()
    }
    async fn send(&self, request: Request<Body>) -> Reply {
        let response = router(self.s.host().clone(), true)
            .layer(Extension(self.s.connection.clone()))
            .oneshot(request)
            .await
            .unwrap();
        let (parts, body) = response.into_parts();
        Reply {
            status: parts.status,
            headers: parts.headers,
            body: to_bytes(body, 4 * MIB).await.unwrap().to_vec(),
        }
    }
    /// Uploads `bytes` as `name`, with a Content-Length of `length`.
    async fn upload_as(&self, name: &str, length: Option<u64>, bytes: Vec<u8>) -> Reply {
        self.send(self.request(
            "POST",
            &format!("/files?name={name}"),
            length,
            Body::from(bytes),
        ))
        .await
    }
    async fn upload(&self, name: &str, bytes: Vec<u8>) -> Reply {
        let length = bytes.len() as u64;
        self.upload_as(name, Some(length), bytes).await
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Another paired device, `uuid`, with `perm`.
fn device(f: &Fixture, uuid: &str, perm: u32) -> Connection {
    let identity = Identity::generate().unwrap();
    let client = Client {
        name: uuid.into(),
        cert: identity.certificate.clone(),
        uuid: uuid.into(),
        perm,
        enabled: true,
        extra: Default::default(),
    };
    f.host
        .paired
        .write()
        .unwrap()
        .add(&f.host.paired_path, client)
        .unwrap();
    Connection {
        certificate: Some(identity.der.clone()),
        ..f.connection(true)
    }
}

/// Waits up to 5 s for `done`.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{what} did not happen within 5 s");
}

impl Files {
    /// Starts a request as `connection`, leaving its answer's body unread.
    async fn start(&self, connection: &Connection, request: Request<Body>) -> Response {
        router(self.s.host().clone(), true)
            .layer(Extension(connection.clone()))
            .oneshot(request)
            .await
            .unwrap()
    }
    /// A file of `size` bytes in the folder.
    fn put(&self, name: &str, size: usize) -> Vec<u8> {
        let bytes: Vec<u8> = (0..size).map(|i| (i % 249) as u8).collect();
        std::fs::create_dir_all(&self.dir).unwrap();
        std::fs::write(self.dir.join(name), &bytes).unwrap();
        bytes
    }
    fn download(&self, name: &str) -> Request<Body> {
        self.request(
            "GET",
            &format!("/files/content?name={name}"),
            None,
            Body::empty(),
        )
    }
    /// An upload of `length` bytes as `name`, its body written to the
    /// returned writer.
    fn streamed(&self, name: &str, length: u64) -> (Request<Body>, tokio::io::DuplexStream) {
        let (writer, reader) = tokio::io::duplex(64 * 1024);
        let request = self.request(
            "POST",
            &format!("/files?name={name}"),
            Some(length),
            Body::from_stream(tokio_util::io::ReaderStream::new(reader)),
        );
        (request, writer)
    }
    fn transfers(&self) -> &butterpollo_stream::file_transfers::Transfers {
        &self.s.host().file_transfers
    }
    async fn free_bytes(&self) -> u64 {
        let reply = self.s.get("/files").await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
        serde_json::from_slice::<FileList>(&reply.body)
            .unwrap()
            .free_bytes
    }
}

#[path = "files/pacing_and_limits_tests.rs"]
mod pacing_and_limits_tests;
#[path = "files/transfer_tests.rs"]
mod transfer_tests;
