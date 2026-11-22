//! Shared blocking HTTP helpers (ureq) for the transcriber and VOICEVOX.

use std::time::Duration;

use anyhow::{Result, bail};

/// Trusts the system's certificates (not only Mozilla's): behind a company or
/// sandbox proxy that re-terminates TLS, the OS knows its CA.
pub fn agent(timeout: Duration) -> ureq::Agent {
    builder(timeout).build().into()
}

/// Keeps unused connections open for `keep`.
pub fn agent_keeping(timeout: Duration, keep: Duration) -> ureq::Agent {
    builder(timeout).max_idle_age(keep).build().into()
}

fn builder(timeout: Duration) -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
}

/// Fails with the status and (truncated) body unless the response is 2xx.
pub fn check(
    mut resp: ureq::http::Response<ureq::Body>,
    what: &str,
) -> Result<ureq::http::Response<ureq::Body>> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let body = resp.body_mut().read_to_string().unwrap_or_default();
    let body: String = body.chars().take(300).collect();
    bail!("{what}: HTTP {status}: {body}");
}

/// Builds a `multipart/form-data` body.
pub struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Multipart {
    pub fn new(boundary: impl Into<String>) -> Self {
        Self {
            boundary: boundary.into(),
            body: Vec::new(),
        }
    }

    pub fn text(mut self, name: &str, value: &str) -> Self {
        self.part_header(name, None, None);
        self.body.extend_from_slice(value.as_bytes());
        self.body.extend_from_slice(b"\r\n");
        self
    }

    pub fn file(mut self, name: &str, filename: &str, content_type: &str, data: &[u8]) -> Self {
        self.part_header(name, Some(filename), Some(content_type));
        self.body.extend_from_slice(data);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    fn part_header(&mut self, name: &str, filename: Option<&str>, content_type: Option<&str>) {
        let mut h = format!(
            "--{}\r\nContent-Disposition: form-data; name=\"{name}\"",
            self.boundary
        );
        if let Some(f) = filename {
            h.push_str(&format!("; filename=\"{f}\""));
        }
        h.push_str("\r\n");
        if let Some(ct) = content_type {
            h.push_str(&format!("Content-Type: {ct}\r\n"));
        }
        h.push_str("\r\n");
        self.body.extend_from_slice(h.as_bytes());
    }

    pub fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        self.body
    }
}

#[cfg(test)]
pub mod fake {
    //! A tiny HTTP/1.1 server for tests: one request per connection.

    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    pub struct Request {
        pub method: String,
        pub target: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Request {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    pub struct Response {
        pub status: u16,
        pub content_type: &'static str,
        pub body: Vec<u8>,
    }

    impl Response {
        pub fn json(body: &str) -> Self {
            Self {
                status: 200,
                content_type: "application/json",
                body: body.as_bytes().to_vec(),
            }
        }
    }

    pub struct Server {
        pub url: String,
        pub requests: Arc<Mutex<Vec<Request>>>,
    }

    pub fn serve(handler: impl Fn(&Request) -> Response + Send + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let target = parts.next().unwrap_or_default().to_string();
                let mut headers = Vec::new();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        headers.push((k.trim().to_string(), v.trim().to_string()));
                    }
                }
                let len = headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let req = Request {
                    method,
                    target,
                    headers,
                    body,
                };
                let resp = handler(&req);
                log.lock().unwrap().push(req);
                let head = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    resp.status,
                    resp.content_type,
                    resp.body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&resp.body);
            }
        });
        Server { url, requests }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_layout() {
        let m = Multipart::new("XX").text("model", "whisper-1").file(
            "file",
            "a.wav",
            "audio/wav",
            b"RIFF",
        );
        assert_eq!(m.content_type(), "multipart/form-data; boundary=XX");
        let body = String::from_utf8(m.finish()).unwrap();
        assert_eq!(
            body,
            "--XX\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nwhisper-1\r\n\
             --XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
             Content-Type: audio/wav\r\n\r\nRIFF\r\n--XX--\r\n"
        );
    }

    #[test]
    fn check_reports_status_and_body() {
        let server = fake::serve(|_| fake::Response {
            status: 401,
            content_type: "text/plain",
            body: b"bad key".to_vec(),
        });
        let resp = agent(Duration::from_secs(5))
            .get(&server.url)
            .call()
            .unwrap();
        let err = check(resp, "stt").unwrap_err().to_string();
        assert_eq!(err, "stt: HTTP 401 Unauthorized: bad key");
    }
}
