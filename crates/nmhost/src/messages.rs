//! The browser side of the protocol (SPEC-browser-capture §5). Each message is a
//! 4-byte little-endian length followed by that many bytes of UTF-8 JSON, and every
//! JSON object carries `"v": 1` and a `"type"`.

use std::io;

use serde::{Deserialize, Serialize};
use slipstream_core::ipc::CaptureSettings;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Protocol version spoken with the extension.
pub const VERSION: u64 = 1;
/// Browsers could send up to 4 GiB; a message this big is a bug, not a download.
const MAX_IN: usize = 64 << 20;
/// Chrome refuses host → browser messages over 1 MiB.
const MAX_OUT: usize = 1 << 20;

/// Extension → host.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Incoming {
    Hello {
        #[serde(default)]
        browser: String,
        #[serde(default)]
        ext: String,
    },
    Capture(Capture),
}

/// A download the browser is about to start, with everything needed to repeat the
/// request as the browser would (cookies, referer, user agent...).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capture {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub modifiers: Modifiers,
}

/// Keys held while clicking: Alt forces a catch, Ctrl prevents it.
#[derive(Debug, Default, Deserialize)]
pub struct Modifiers {
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub prevent: bool,
}

/// Host → extension.
#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Outgoing {
    Settings(CaptureSettings),
    Verdict {
        id: String,
        action: Action,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Slipstream downloads it; the browser cancels its own download.
    Take,
    /// The browser keeps it.
    Release,
}

/// Every outgoing message, with the protocol version added.
#[derive(Serialize)]
struct Envelope<'a> {
    v: u64,
    #[serde(flatten)]
    msg: &'a Outgoing,
}

/// Decode one message, checking the protocol version.
pub fn parse(raw: &[u8]) -> Result<Incoming, String> {
    let value: serde_json::Value = serde_json::from_slice(raw).map_err(|e| e.to_string())?;
    match value.get("v").and_then(|v| v.as_u64()) {
        Some(VERSION) => serde_json::from_value(value).map_err(|e| e.to_string()),
        other => Err(format!(
            "unsupported protocol version {other:?}; this host speaks v{VERSION}"
        )),
    }
}

/// Read one framed message; `None` when the browser closed the port.
pub async fn read(r: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_IN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("message of {len} bytes"),
        ));
    }
    let mut buf = vec![0; len];
    r.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

/// Write one framed message.
pub async fn write(w: &mut (impl AsyncWrite + Unpin), msg: &Outgoing) -> io::Result<()> {
    let json = serde_json::to_vec(&Envelope { v: VERSION, msg })?;
    if json.len() > MAX_OUT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "reply too big for the browser",
        ));
    }
    w.write_all(&(json.len() as u32).to_le_bytes()).await?;
    w.write_all(&json).await?;
    w.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hello_and_capture() {
        let hello = parse(br#"{"v":1,"type":"hello","browser":"Chrome/141","ext":"0.1.0"}"#);
        assert!(matches!(hello, Ok(Incoming::Hello { browser, .. }) if browser == "Chrome/141"));

        let capture = parse(
            br#"{"v":1,"type":"capture","id":"c-1","url":"https://x/a.zip",
                 "headers":[["Cookie","a=b"]],"filename":"a.zip","mime":"application/zip",
                 "modifiers":{"force":false,"prevent":true}}"#,
        );
        let Ok(Incoming::Capture(c)) = capture else {
            panic!("{capture:?}")
        };
        assert_eq!((c.id.as_str(), c.url.as_str()), ("c-1", "https://x/a.zip"));
        assert_eq!(c.headers, vec![("Cookie".to_owned(), "a=b".to_owned())]);
        assert!(c.modifiers.prevent);

        assert!(
            parse(br#"{"v":2,"type":"hello"}"#)
                .unwrap_err()
                .contains("version")
        );
        assert!(parse(b"not json").is_err());
    }

    #[test]
    fn replies_carry_version_and_type() {
        let msg = Outgoing::Verdict {
            id: "c-1".into(),
            action: Action::Take,
        };
        let json = serde_json::to_string(&Envelope {
            v: VERSION,
            msg: &msg,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"v":1,"type":"verdict","id":"c-1","action":"take"}"#
        );
    }

    #[tokio::test]
    async fn framing_round_trip() {
        let msg = Outgoing::Error {
            id: None,
            message: "x".into(),
        };
        let mut buf = Vec::new();
        write(&mut buf, &msg).await.unwrap();
        assert_eq!(
            u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize,
            buf.len() - 4
        );
        let mut r = &buf[..];
        let back = read(&mut r).await.unwrap().unwrap();
        assert_eq!(back, br#"{"v":1,"type":"error","message":"x"}"#);
        assert!(read(&mut r).await.unwrap().is_none());
    }
}
