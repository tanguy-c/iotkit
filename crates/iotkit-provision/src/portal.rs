//! The setup page of the access point: routes, form decoding, HTML.

use core::fmt::Write;

use heapless::String;

/// What a request asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// The form.
    Page,
    /// The form was sent.
    Connect,
    /// Anything else (e.g. a phone's connectivity check): redirect to the form, which makes the
    /// phone show the portal.
    Redirect,
}

/// What to answer to a request.
pub fn route(method: &str, path: &str) -> Route {
    match (method, path) {
        ("GET", "/") => Route::Page,
        ("POST", "/connect") => Route::Connect,
        _ => Route::Redirect,
    }
}

/// `Content-Length` of a request head (status line and headers, without the blank line).
pub fn content_length(head: &str) -> Option<usize> {
    head.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse().ok())?
    })
}

/// `ssid` and `password` from an `application/x-www-form-urlencoded` body.
pub fn parse_form(body: &[u8]) -> Option<(String<32>, String<64>)> {
    let body = core::str::from_utf8(body).ok()?;
    let (mut ssid, mut password) = (None, None);
    for pair in body.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "ssid" => ssid = Some(decode::<32>(value)?),
            "password" => password = Some(decode::<64>(value)?),
            _ => {}
        }
    }
    let ssid = ssid.filter(|s| !s.is_empty())?;
    Some((ssid, password.unwrap_or_default()))
}

/// Percent-decoding, `+` for space.
fn decode<const N: usize>(s: &str) -> Option<String<N>> {
    let mut bytes = heapless::Vec::<u8, N>::new();
    let mut it = s.bytes();
    while let Some(b) = it.next() {
        let byte = match b {
            b'+' => b' ',
            b'%' => {
                let hex = |c: u8| (c as char).to_digit(16);
                (hex(it.next()?)? * 16 + hex(it.next()?)?) as u8
            }
            b => b,
        };
        bytes.push(byte).ok()?;
    }
    String::from_utf8(bytes).ok()
}

/// Writes `s` with HTML special characters escaped.
fn escaped(out: &mut impl Write, s: &str) -> core::fmt::Result {
    for c in s.chars() {
        match c {
            '<' => out.write_str("&lt;")?,
            '>' => out.write_str("&gt;")?,
            '&' => out.write_str("&amp;")?,
            '"' => out.write_str("&quot;")?,
            c => out.write_char(c)?,
        }
    }
    Ok(())
}

/// The outcome shown above the form.
pub enum Notice<'a> {
    /// Nothing to show.
    None,
    /// Joining: the page reloads itself until there is an outcome. (Joining restarts the access
    /// point, so the answer cannot wait for it.)
    Trying {
        /// The network being joined.
        ssid: &'a str,
    },
    /// Joined: the device's address there.
    Joined {
        /// The network joined.
        ssid: &'a str,
        /// The device's URL there.
        url: &'a str,
    },
    /// Joining failed: the form is shown again.
    Failed {
        /// The network that failed.
        ssid: &'a str,
    },
}

/// The page: a message, then the form unless joined.
pub fn page<const N: usize>(name: &str, notice: Notice) -> String<N> {
    let mut s = String::new();
    let _ = (|| -> core::fmt::Result {
        s.write_str(
            "<!doctype html><html><head><meta charset=utf-8>\
             <meta name=viewport content=\"width=device-width,initial-scale=1\">",
        )?;
        if matches!(notice, Notice::Trying { .. }) {
            s.write_str("<meta http-equiv=refresh content=\"2;url=/\">")?;
        }
        s.write_str("<title>")?;
        escaped(&mut s, name)?;
        s.write_str(
            "</title><style>body{font-family:sans-serif;max-width:24em;margin:2em auto;padding:0 1em}\
             input,button{width:100%;padding:.6em;margin:.3em 0 1em;box-sizing:border-box;font-size:1em}\
             .ok{color:#080}.err{color:#b00}</style></head><body><h1>",
        )?;
        escaped(&mut s, name)?;
        s.write_str("</h1>")?;
        match notice {
            Notice::None => {}
            Notice::Trying { ssid } => {
                s.write_str("<p>Joining <b>")?;
                escaped(&mut s, ssid)?;
                return s.write_str(
                    "</b>&hellip; This page reloads by itself; if this network drops for a moment, \
                     rejoin it.</p></body></html>",
                );
            }
            Notice::Joined { ssid, url } => {
                s.write_str("<p class=ok>Connected to <b>")?;
                escaped(&mut s, ssid)?;
                s.write_str("</b>. Rejoin your usual network; the device is at <a href=\"")?;
                escaped(&mut s, url)?;
                s.write_str("\">")?;
                escaped(&mut s, url)?;
                return s.write_str("</a>.</p></body></html>");
            }
            Notice::Failed { ssid } => {
                s.write_str("<p class=err>Could not join <b>")?;
                escaped(&mut s, ssid)?;
                s.write_str("</b>: check the password (8 to 63 characters; 2.4 GHz networks only).</p>")?;
            }
        }
        s.write_str(
            "<form method=post action=/connect><label>Wi-Fi network<input name=ssid maxlength=32 required></label>\
             <label>Password<input name=password type=password maxlength=64 pattern=\".{8,64}\" title=\"8 to 63 characters, or empty for an open network\"></label>\
             <button>Connect</button></form></body></html>",
        )
    })();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes() {
        assert_eq!(route("GET", "/"), Route::Page);
        assert_eq!(route("POST", "/connect"), Route::Connect);
        assert_eq!(route("GET", "/generate_204"), Route::Redirect);
        assert_eq!(route("GET", "/hotspot-detect.html"), Route::Redirect);
    }

    #[test]
    fn request_lengths() {
        assert_eq!(content_length("POST /connect HTTP/1.1\r\nHost: x\r\ncontent-length: 27"), Some(27));
        assert_eq!(content_length("GET / HTTP/1.1\r\nHost: x"), None);
        assert_eq!(content_length("POST / HTTP/1.1\r\nContent-Length: x"), None);
    }

    #[test]
    fn forms() {
        let (ssid, pw) = parse_form(b"ssid=My+Home%21&password=p%40ss+w%C3%B6rd").unwrap();
        assert_eq!((ssid.as_str(), pw.as_str()), ("My Home!", "p@ss wörd"));
        let (ssid, pw) = parse_form(b"password=&ssid=Open").unwrap();
        assert_eq!((ssid.as_str(), pw.as_str()), ("Open", ""));
        assert_eq!(parse_form(b"ssid=&password=x"), None);
        assert_eq!(parse_form(b"password=x"), None);
        assert_eq!(parse_form(b"ssid=%zz"), None);
        assert_eq!(parse_form(b"ssid=%C3"), None, "not UTF-8");
        let long = [b'a'; 40];
        let mut body = heapless::Vec::<u8, 64>::new();
        body.extend_from_slice(b"ssid=").unwrap();
        body.extend_from_slice(&long).unwrap();
        assert_eq!(parse_form(&body), None, "SSID over 32 bytes");
    }

    #[test]
    fn pages_escape_and_fit() {
        let p: String<2048> = page("iotkit-aefc", Notice::Failed { ssid: "<script>" });
        assert!(p.contains("&lt;script&gt;") && !p.contains("<script>"));
        assert!(p.contains("<form") && p.ends_with("</html>"));
        let p: String<2048> = page("iotkit-aefc", Notice::Trying { ssid: "Home" });
        assert!(p.contains("http-equiv=refresh") && !p.contains("<form") && p.ends_with("</html>"));
        let p: String<2048> = page("iotkit-aefc", Notice::Joined { ssid: "Home", url: "http://iotkit-aefc.local/" });
        assert!(p.contains("http://iotkit-aefc.local/") && !p.contains("<form") && p.ends_with("</html>"));
    }
}
