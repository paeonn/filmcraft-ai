//! OpenAI-compatible wire protocol. Never return credentials, response bodies or URLs in errors.
use super::*;
use std::io::Read;
use std::time::Duration;

const MAX_RESPONSE: u64 = 4 * 1024 * 1024;

pub(super) struct Client {
    config: Config,
    key: String,
    agent: ureq::Agent,
}

impl Client {
    pub(super) fn new(config: Config, key: String) -> Result<Self> {
        let uri = config.base_url.parse::<ureq::http::Uri>().map_err(|_| err("invalid gateway URL"))?;
        if uri.authority().is_none() {
            return Err(err("invalid gateway host"));
        }
        let agent = ureq::Agent::config_builder()
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::Rustls)
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .unversioned_rustls_crypto_provider(Arc::new(rustls_rustcrypto::provider()))
                    .build(),
            )
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(120)))
            .max_redirects(0)
            .build()
            .into();
        Ok(Self { config, key, agent })
    }

    fn read(&self, response: ureq::http::Response<ureq::Body>, cancel: &AtomicBool) -> std::result::Result<Value, String> {
        if !response.status().is_success() {
            return Err(format!("OmniRoute HTTP {}; check gateway credentials and model capabilities", response.status().as_u16()));
        }
        let mut body = response.into_body();
        let mut reader = body.as_reader().take(MAX_RESPONSE + 1);
        let mut data = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("AI request cancelled".into());
            }
            let n = reader.read(&mut buf).map_err(|_| "could not read OmniRoute response")?;
            if n == 0 {
                break;
            }
            data.extend_from_slice(buf.get(..n).ok_or("invalid response read size")?);
            if data.len() as u64 > MAX_RESPONSE {
                return Err("OmniRoute response exceeds 4 MiB".into());
            }
        }
        serde_json::from_slice(&data).map_err(|_| "OmniRoute returned invalid JSON".into())
    }
}

fn network_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(code) => format!("OmniRoute HTTP {code}; check credentials, model capabilities and rate limits"),
        _ => "OmniRoute request failed; check gateway address, TLS and timeout (120 seconds)".into(),
    }
}

impl Backend for Client {
    fn request(&self, request: Request, cancel: &AtomicBool) -> std::result::Result<Value, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("AI request cancelled".into());
        }
        let authorization = format!("Bearer {}", self.key);
        let response = match request {
            Request::Models => self.agent.get(format!("{}/models", self.config.base_url)).header("Authorization", &authorization).call(),
            Request::Chat { system, input } => {
                if self.config.chat_model.is_empty() {
                    return Err("configure chat_model before requesting text or graphics".into());
                }
                let body = json!({"model": self.config.chat_model, "stream": false, "max_tokens": 8192,
                    "response_format":{"type":"json_object"}, "messages":[{"role":"system","content":system},{"role":"user","content":input.to_string()}]})
                .to_string();
                self.agent
                    .post(format!("{}/chat/completions", self.config.base_url))
                    .header("Authorization", &authorization)
                    .header("Content-Type", "application/json")
                    .send(body.as_bytes())
            }
            Request::Audio { wav, language } => {
                let (body, boundary) = multipart(&self.config.speech_model, language.as_deref(), &wav)?;
                self.agent
                    .post(format!("{}/audio/transcriptions", self.config.base_url))
                    .header("Authorization", &authorization)
                    .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
                    .send(body.as_slice())
            }
        }
        .map_err(network_error)?;
        self.read(response, cancel)
    }
}

fn multipart(model: &str, language: Option<&str>, wav: &[u8]) -> std::result::Result<(Vec<u8>, String), String> {
    if model.is_empty()
        || model.len() > 256
        || model.chars().any(|c| c.is_control())
        || language.is_some_and(|l| l.len() > 32 || !l.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-'))
    {
        return Err("invalid speech model or language".into());
    }
    let boundary = (0..16)
        .map(|n| format!("filmcraft-omniroute-boundary-{n:04}"))
        .find(|b| !wav.windows(b.len()).any(|w| w == b.as_bytes()))
        .ok_or("could not encode audio multipart boundary")?;
    let mut out = Vec::new();
    for (name, value) in
        [("model", model), ("response_format", "verbose_json"), ("timestamp_granularities[]", "word")].into_iter().chain(language.map(|l| ("language", l)))
    {
        out.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    out.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
    );
    out.extend_from_slice(wav);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Ok((out, boundary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    fn server(status: u16, body: &'static str) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut data = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&data[..end]);
                    let length: usize =
                        header.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|s| s.trim().parse().unwrap())).unwrap_or(0);
                    if data.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                .unwrap();
            data
        });
        (url, thread)
    }

    #[test]
    fn chat_and_audio_use_openai_contract_and_bearer_auth() {
        let (base_url, server) = server(200, r#"{"choices":[{"message":{"content":"{}"}}]}"#);
        let client = Client::new(Config { base_url, chat_model: "test/chat".into(), ..Default::default() }, "synthetic-key".into()).unwrap();
        client.request(Request::Chat { system: "system".into(), input: json!({"instruction":"edit"}) }, &AtomicBool::new(false)).unwrap();
        let bytes = server.join().unwrap();
        let wire = String::from_utf8_lossy(&bytes);
        assert!(wire.starts_with("POST /v1/chat/completions "));
        assert!(wire.to_ascii_lowercase().contains("authorization: bearer synthetic-key"));
        assert!(wire.contains("json_object"));

        let (base_url, server) = self::server(200, r#"{"words":[]}"#);
        let client = Client::new(Config { base_url, speech_model: "test/whisper".into(), ..Default::default() }, String::new()).unwrap();
        client.request(Request::Audio { wav: wav(&[0.0, 0.5]).unwrap(), language: Some("pt".into()) }, &AtomicBool::new(false)).unwrap();
        let bytes = server.join().unwrap();
        let wire = String::from_utf8_lossy(&bytes);
        assert!(wire.starts_with("POST /v1/audio/transcriptions "));
        assert!(wire.contains("timestamp_granularities[]"));
        assert!(wire.contains("verbose_json"));
        assert!(wire.contains("RIFF"));
    }

    #[test]
    fn provider_errors_are_sanitized_and_redirects_are_not_followed() {
        for code in [401, 429, 500, 302] {
            let (base_url, server) = server(code, r#"{"error":"provider echoed secret-value"}"#);
            let client = Client::new(Config { base_url, ..Default::default() }, "secret-value".into()).unwrap();
            let error = client.request(Request::Models, &AtomicBool::new(false)).unwrap_err();
            assert!(!error.contains("secret-value"));
            assert!(error.contains(&code.to_string()));
            server.join().unwrap();
        }
    }

    #[test]
    fn malformed_json_and_multipart_injection_are_rejected() {
        let (base_url, server) = server(200, "not json");
        let client = Client::new(Config { base_url, ..Default::default() }, String::new()).unwrap();
        assert!(client.request(Request::Models, &AtomicBool::new(false)).is_err());
        server.join().unwrap();
        assert!(multipart("model\r\ninjection", None, &[]).is_err());
        assert!(multipart("model", Some("pt\r\n"), &[]).is_err());
    }
}
