//! Just enough HTTP/1.1: `http://` URLs, response heads for a GET client, and request lines for
//! a tiny server. Bodies are framed by `Content-Length` (or the connection closing); chunked
//! transfer encoding is not supported. HTTPS comes later: a signed manifest makes plain HTTP safe
//! for updates, though not private.

use core::fmt::Write;

use heapless::String;

/// Longest URL kept.
pub const MAX_URL: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not `http://host[:port]/path`.
    Url,
    /// `https://`, or a chunked response.
    Unsupported,
    /// Not an HTTP response head, or it is larger than the buffer.
    BadResponse,
}

/// A parsed `http://` URL, borrowing from the string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Url<'a> {
    pub host: &'a str,
    pub port: u16,
    /// Starts with `/`.
    pub path: &'a str,
}

impl<'a> Url<'a> {
    pub fn parse(url: &'a str) -> Result<Self, Error> {
        // Control characters would end up in the request head (CR/LF: an injected header).
        if url.bytes().any(|b| b < 0x20 || b == 0x7f) {
            return Err(Error::Url);
        }
        if url.starts_with("https://") {
            return Err(Error::Unsupported);
        }
        let rest = url.strip_prefix("http://").ok_or(Error::Url)?;
        let (authority, path) = rest.find('/').map_or((rest, "/"), |i| rest.split_at(i));
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().map_err(|_| Error::Url)?),
            None => (authority, 80),
        };
        if host.is_empty() || host.contains(['@', ' ', '?', '#']) || path.contains(' ') {
            return Err(Error::Url);
        }
        Ok(Self { host, port, path })
    }

    /// `reference` resolved against this URL: absolute, absolute-path, or relative to this
    /// path's directory (`a/b.json` + `c.bin` = `a/c.bin`).
    pub fn join(&self, reference: &str) -> Result<String<MAX_URL>, Error> {
        let mut out = String::new();
        if reference.contains("://") {
            Url::parse(reference)?;
            out.push_str(reference).map_err(|_| Error::Url)?;
            return Ok(out);
        }
        write!(out, "http://{}:{}", self.host, self.port).map_err(|_| Error::Url)?;
        if !reference.starts_with('/') {
            let path = self.path.split(['?', '#']).next().unwrap_or("/");
            let dir = &path[..=path.rfind('/').unwrap_or(0)];
            out.push_str(dir).map_err(|_| Error::Url)?;
        }
        out.push_str(reference).map_err(|_| Error::Url)?;
        Url::parse(&out)?;
        Ok(out)
    }
}

/// A response's status line and framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: u16,
    /// `None`: the body ends when the connection closes.
    pub content_length: Option<u32>,
    /// Bytes of the head, `\r\n\r\n` included; the body follows.
    pub len: usize,
}

/// Parses a response head at the start of `buf`; `Ok(None)` if it is not complete yet.
pub fn parse_response_head(buf: &[u8]) -> Result<Option<ResponseHead>, Error> {
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Ok(None);
    };
    let head = core::str::from_utf8(&buf[..end]).map_err(|_| Error::BadResponse)?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or(Error::BadResponse)?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().ok_or(Error::BadResponse)?;
    if !version.starts_with("HTTP/1.") {
        return Err(Error::BadResponse);
    }
    let status = parts.next().and_then(|s| s.parse().ok()).ok_or(Error::BadResponse)?;
    let mut content_length = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(Error::BadResponse)?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(value.parse().map_err(|_| Error::BadResponse)?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") && !value.eq_ignore_ascii_case("identity") {
            return Err(Error::Unsupported);
        }
    }
    Ok(Some(ResponseHead { status, content_length, len: end + 4 }))
}

/// Writes a GET request for `url` into `out`; returns its length. `from`: ask for the body from
/// that byte on (a `Range` request, answered with `206 Partial Content`).
pub fn write_get(url: &Url, from: u32, out: &mut [u8]) -> Result<usize, Error> {
    struct W<'a>(&'a mut [u8], usize);
    impl Write for W<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let end = self.1 + s.len();
            self.0.get_mut(self.1..end).ok_or(core::fmt::Error)?.copy_from_slice(s.as_bytes());
            self.1 = end;
            Ok(())
        }
    }
    let mut w = W(out, 0);
    let host_port = |w: &mut W| {
        if url.port == 80 { write!(w, "{}", url.host) } else { write!(w, "{}:{}", url.host, url.port) }
    };
    (|| {
        write!(w, "GET {} HTTP/1.1\r\nHost: ", url.path)?;
        host_port(&mut w)?;
        if from > 0 {
            write!(w, "\r\nRange: bytes={from}-")?;
        }
        write!(w, "\r\nUser-Agent: iotkit\r\nConnection: close\r\n\r\n")
    })()
    .map_err(|_| Error::Url)?;
    Ok(w.1)
}

/// The method and target of a request line (`GET /ota?x=y HTTP/1.1`).
pub fn parse_request_line(line: &str) -> Option<(&str, &str)> {
    let mut parts = line.split(' ');
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    (version.starts_with("HTTP/1.") && target.starts_with('/') && parts.next().is_none()).then_some((method, target))
}

/// The path of a request target, without the query.
pub fn target_path(target: &str) -> &str {
    target.split_once('?').map_or(target, |(path, _)| path)
}

/// Raw (not percent-decoded) value of query parameter `name` in a request target. The last
/// parameter may contain `&`-free URLs as they are, e.g. `/ota?manifest=http://pc:8000/m.json`.
pub fn query_param<'a>(target: &'a str, name: &str) -> Option<&'a str> {
    let (_, query) = target.split_once('?')?;
    query.split('&').find_map(|pair| pair.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            Url::parse("http://10.0.0.2:8080/devkit/manifest.json"),
            Ok(Url { host: "10.0.0.2", port: 8080, path: "/devkit/manifest.json" })
        );
        assert_eq!(Url::parse("http://pc.local"), Ok(Url { host: "pc.local", port: 80, path: "/" }));
        assert_eq!(Url::parse("https://x/y"), Err(Error::Unsupported));
        for bad in [
            "ftp://x/",
            "http://",
            "http://:80/",
            "http://x:port/",
            "http://x:99999/",
            "http://u@x/",
            "x/y",
            "http://x/a\r\nHost: y",
        ] {
            assert_eq!(Url::parse(bad), Err(Error::Url), "{bad}");
        }
    }

    #[test]
    fn joins() {
        let base = Url::parse("http://10.0.0.2:8080/devkit/manifest.json?v=1").unwrap();
        assert_eq!(base.join("devkit-1.2.0.bin").unwrap(), "http://10.0.0.2:8080/devkit/devkit-1.2.0.bin");
        assert_eq!(base.join("/other/x.bin").unwrap(), "http://10.0.0.2:8080/other/x.bin");
        assert_eq!(base.join("http://mirror/x.bin").unwrap(), "http://mirror/x.bin");
        assert_eq!(base.join("https://mirror/x.bin"), Err(Error::Unsupported));
        let root = Url::parse("http://h").unwrap();
        assert_eq!(root.join("a.bin").unwrap(), "http://h:80/a.bin");
    }

    #[test]
    fn response_heads() {
        let r = b"HTTP/1.1 200 OK\r\nServer: Caddy\r\ncontent-length: 1234\r\n\r\nBODY";
        assert_eq!(
            parse_response_head(r),
            Ok(Some(ResponseHead { status: 200, content_length: Some(1234), len: r.len() - 4 }))
        );
        assert_eq!(parse_response_head(b"HTTP/1.0 404 Not Found\r\n\r\n").unwrap().unwrap().status, 404);
        assert_eq!(parse_response_head(b"HTTP/1.1 200 OK\r\nContent-Le"), Ok(None));
        assert_eq!(
            parse_response_head(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Err(Error::Unsupported)
        );
        assert_eq!(parse_response_head(b"SSH-2.0\r\n\r\n"), Err(Error::BadResponse));
        assert_eq!(parse_response_head(b"HTTP/1.1 OK\r\n\r\n"), Err(Error::BadResponse));
    }

    #[test]
    fn get_request() {
        let mut out = [0; 128];
        let n = write_get(&Url::parse("http://10.0.0.2:8080/m.json").unwrap(), 0, &mut out).unwrap();
        assert_eq!(
            core::str::from_utf8(&out[..n]).unwrap(),
            "GET /m.json HTTP/1.1\r\nHost: 10.0.0.2:8080\r\nUser-Agent: iotkit\r\nConnection: close\r\n\r\n"
        );
        let n = write_get(&Url::parse("http://h/").unwrap(), 0, &mut out).unwrap();
        assert!(core::str::from_utf8(&out[..n]).unwrap().contains("Host: h\r\n"));
        let n = write_get(&Url::parse("http://h/x.bin").unwrap(), 4096, &mut out).unwrap();
        assert_eq!(
            core::str::from_utf8(&out[..n]).unwrap(),
            "GET /x.bin HTTP/1.1\r\nHost: h\r\nRange: bytes=4096-\r\nUser-Agent: iotkit\r\nConnection: close\r\n\r\n"
        );
        assert_eq!(write_get(&Url::parse("http://h/").unwrap(), 0, &mut [0; 10]), Err(Error::Url));
    }

    #[test]
    fn requests() {
        let line = "GET /ota?manifest=http://10.0.0.2:8000/devkit/manifest.json HTTP/1.1";
        let (method, target) = parse_request_line(line).unwrap();
        assert_eq!((method, target_path(target)), ("GET", "/ota"));
        assert_eq!(query_param(target, "manifest"), Some("http://10.0.0.2:8000/devkit/manifest.json"));
        assert_eq!(query_param(target, "other"), None);
        assert_eq!(query_param("/ota", "manifest"), None);
        assert_eq!(query_param("/x?a=1&b=2", "b"), Some("2"));
        assert_eq!(parse_request_line("GET / HTTP/1.1 extra"), None);
        assert_eq!(parse_request_line("GET x HTTP/1.1"), None);
        assert_eq!(parse_request_line("hello"), None);
    }
}
