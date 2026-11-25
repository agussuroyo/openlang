use std::time::Duration;

use openlang_core::{Error, Result};
use serde_json::Value;
use ureq::Agent;
use url::{Host, Url};

pub struct Client {
    agent: Agent,
    base: String,
    key: String,
}

pub fn is_loopback(base: &str) -> bool {
    match Url::parse(base.trim()).ok().as_ref().and_then(Url::host) {
        Some(Host::Domain(d)) => d == "localhost",
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

fn failed(e: ureq::Error) -> Error {
    Error::Provider(e.to_string())
}

impl Client {
    pub fn new(base: &str, key: &str) -> Result<Self> {
        let url = Url::parse(base).map_err(|e| Error::Config(format!("server address: {e}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::Config(
                "the server address must be http or https".into(),
            ));
        }
        if !key.is_empty() && url.scheme() == "http" && !is_loopback(base) {
            return Err(Error::Config("the API key is only sent over https".into()));
        }
        // A redirect could carry speech or the key to a host the user never agreed to.
        let agent = Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        Ok(Self {
            agent,
            base: base.trim_end_matches('/').to_owned(),
            key: key.to_owned(),
        })
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.send(path, "application/json", body.to_string().as_bytes())
    }

    pub fn upload(&self, path: &str, fields: &[(&str, &str)], wav: &[u8]) -> Result<Value> {
        const BOUNDARY: &str = "openlang-7c2f1b9e54d84a6b";
        let mut body = Vec::new();
        for (name, value) in fields {
            let value = value.replace(['\r', '\n'], "");
            body.extend_from_slice(
                format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                    .as_bytes(),
            );
        }
        body.extend_from_slice(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(wav);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        self.send(
            path,
            &format!("multipart/form-data; boundary={BOUNDARY}"),
            &body,
        )
    }

    fn send(&self, path: &str, kind: &str, body: &[u8]) -> Result<Value> {
        let mut request = self
            .agent
            .post(format!("{}/{path}", self.base))
            .header("Content-Type", kind);
        if !self.key.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.key));
        }
        let mut reply = request.send(body).map_err(failed)?;
        let status = reply.status();
        let text = reply.body_mut().read_to_string().map_err(failed)?;
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            let detail = value["error"]["message"].as_str().unwrap_or_default();
            return Err(Error::Provider(format!(
                "the server answered {status} {detail}"
            )));
        }
        Ok(value)
    }
}
