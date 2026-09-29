use axum::http::{header::COOKIE, HeaderMap, HeaderValue};

pub const SESSION_COOKIE: &str = "dockerer_session";

pub fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(';'))
        .find_map(|pair| {
            let (k, v) = pair.trim().split_once('=')?;
            (k == name).then(|| v.to_string())
        })
}

pub fn session_cookie(token: &str, max_age: i64, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"
    ))
    .expect("token is hex, so the cookie is a valid header value")
}

pub fn clear_cookie(secure: bool) -> HeaderValue {
    session_cookie("", 0, secure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::COOKIE;

    #[test]
    fn reads_named_cookie_across_headers() {
        let mut h = HeaderMap::new();
        h.append(COOKIE, "a=1; other=2".parse().unwrap());
        h.append(COOKIE, "b=3; dockerer_session=abc".parse().unwrap());
        assert_eq!(read_cookie(&h, SESSION_COOKIE).as_deref(), Some("abc"));
        assert_eq!(read_cookie(&h, "missing"), None);
    }

    #[test]
    fn cookie_flags() {
        assert_eq!(
            session_cookie("tok", 60, false),
            "dockerer_session=tok; Path=/; HttpOnly; SameSite=Strict; Max-Age=60"
        );
        assert!(session_cookie("tok", 60, true)
            .to_str()
            .unwrap()
            .ends_with("; Secure"));
        assert!(clear_cookie(false).to_str().unwrap().contains("Max-Age=0"));
    }
}
