//! Browser session authentication, sign-in rate limiting, CSRF checks, and auth/password endpoints.
use super::{error, response};
use axum::{
    Json,
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::Engine;
use butterpollo_crypto::{auth, crypto};
use butterpollo_session::state::Credentials;
use butterpollo_stream::{state::Shared, tls::Connection, web_sessions};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(crate) fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|c| {
            let (k, v) = c.trim().split_once('=')?;
            if k == name { Some(v.to_owned()) } else { None }
        })
}

pub(crate) fn access(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned)
        .or_else(|| cookie(headers, "__Host-apollo_session"))
}

/// Sign-in attempts per address over the last minute. The login form and
/// Basic credentials share it, so neither lets a peer guess the password at
/// full speed. This PC is never throttled, and IPv6 counts per /64, the
/// block one LAN host can take addresses from.
pub(super) struct SignIns {
    pub(super) attempts: std::collections::HashMap<std::net::IpAddr, (Instant, u32)>,
}

static SIGN_INS: std::sync::LazyLock<std::sync::Mutex<SignIns>> = std::sync::LazyLock::new(|| {
    std::sync::Mutex::new(SignIns {
        attempts: Default::default(),
    })
});

impl SignIns {
    const LIMIT: u32 = 10;
    const WINDOW: Duration = Duration::from_secs(60);
    pub(super) const TRACKED: usize = 1024;
    fn key(peer: std::net::IpAddr) -> std::net::IpAddr {
        let peer = peer.to_canonical();
        match peer {
            std::net::IpAddr::V6(address) if !address.is_loopback() => {
                std::net::Ipv6Addr::from(u128::from(address) & (u128::MAX << 64)).into()
            }
            other => other,
        }
    }
    /// Whether `peer` may attempt sign-in without counting against the limit.
    pub(super) fn allowed(&mut self, peer: std::net::IpAddr, now: Instant) -> bool {
        let key = Self::key(peer);
        self.attempts
            .retain(|_, (since, _)| now.saturating_duration_since(*since) < Self::WINDOW);
        self.attempts
            .get(&key)
            .is_none_or(|(_, count)| *count < Self::LIMIT)
    }
    /// Record a failed sign-in attempt for `peer`.
    pub(super) fn failed(&mut self, peer: std::net::IpAddr, now: Instant) {
        let key = Self::key(peer);
        self.attempts
            .retain(|_, (since, _)| now.saturating_duration_since(*since) < Self::WINDOW);
        // Full: forget the oldest address instead of refusing every new
        // one, which would let a flood of addresses lock everyone out.
        if self.attempts.len() >= Self::TRACKED
            && !self.attempts.contains_key(&key)
            && let Some(oldest) = self
                .attempts
                .iter()
                .min_by_key(|(_, (since, _))| *since)
                .map(|(address, _)| *address)
        {
            self.attempts.remove(&oldest);
        }
        let (_, count) = self.attempts.entry(key).or_insert((now, 0));
        *count = count.saturating_add(1);
    }
    pub(super) fn succeeded(&mut self, peer: std::net::IpAddr) {
        let key = Self::key(peer);
        self.attempts.remove(&key);
    }
}

fn rehash_if_needed(h: &Shared, username: String, password: &str) {
    match Credentials::new(username, password) {
        Ok(upgraded) => match h.save_credentials(&upgraded) {
            Ok(()) => *h.credentials.write().unwrap() = Some(upgraded),
            Err(error) => tracing::warn!(%error, "could not save upgraded password hash"),
        },
        Err(error) => tracing::warn!(%error, "could not upgrade password hash"),
    }
}

/// Whether the request is signed in: a browser session, or Basic
/// credentials from `peer`, which count against its sign-in attempts.
pub(crate) fn authenticated(
    h: &Shared,
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
) -> bool {
    if let Some(token) = access(headers) {
        let mut sessions = h.web_sessions.lock().unwrap();
        let Some(key) = web_sessions::resolve_hash(&sessions, &token) else {
            return false;
        };
        let Some(session) = sessions
            .get_mut(&key)
            .filter(|s| s.expires > Instant::now())
        else {
            return false;
        };
        let wall = web_sessions::now();
        if wall.saturating_sub(session.last_seen) >= 300 {
            let previous = session.last_seen;
            session.last_seen = wall;
            if let Err(error) = h.save_web_sessions(&sessions) {
                sessions.get_mut(&key).unwrap().last_seen = previous;
                tracing::debug!(%error, "browser session activity could not be saved");
            }
        }
        return true;
    }
    if let Some(encoded) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        && let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded)
        && let Ok(text) = std::str::from_utf8(&decoded)
        && let Some((user, pass)) = text.split_once(':')
    {
        let peer = peer.unwrap_or(std::net::Ipv4Addr::UNSPECIFIED.into());
        let now = Instant::now();
        if !SIGN_INS.lock().unwrap().allowed(peer, now) {
            return false;
        }
        let (verified, upgrade_user) = {
            let guard = h.credentials.read().unwrap();
            match guard.as_ref() {
                Some(c) if c.verifies(user, pass) => {
                    (true, c.needs_rehash().then(|| c.username.clone()))
                }
                _ => (false, None),
            }
        };
        if verified {
            SIGN_INS.lock().unwrap().succeeded(peer);
            if let Some(username) = upgrade_user {
                rehash_if_needed(h, username, pass);
            }
        } else {
            SIGN_INS.lock().unwrap().failed(peer, now);
        }
        return verified;
    }
    false
}

pub(super) fn token_authenticated(
    h: &Shared,
    headers: &HeaderMap,
    path: &str,
    method: &str,
) -> bool {
    let Some(secret) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    let credentials = h.credentials.read().unwrap();
    let Some(credentials) = credentials.as_ref() else {
        return false;
    };
    auth::read(&h.aliases.lock().unwrap())
        .is_ok_and(|tokens| auth::permits(&tokens, secret, &credentials.username, path, method))
}

/// A state-changing request with Basic credentials needs no CSRF token
/// (docs/api.md), and a browser that cached those credentials attaches them
/// to a cross-site form too, with no Origin if the browser or an extension
/// drops it. A form's content type, or the browser's own word that the
/// request is cross-site, gives that away. Scripts send JSON or no body and
/// no `Sec-Fetch-Site`.
pub(super) fn basic_auth_cross_site(headers: &HeaderMap) -> bool {
    let basic = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Basic "));
    let cross_site = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|site| !matches!(site, "same-origin" | "none"));
    let form = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|kind| kind.trim().to_ascii_lowercase())
        .is_some_and(|kind| {
            matches!(
                kind.as_str(),
                "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain"
            )
        });
    basic && (cross_site || form)
}

/// Whether `origin` is the console's own (`https://` and the request's Host) or one of
/// `csrf_allowed_origins`.
pub(super) fn origin_allowed(h: &Shared, headers: &HeaderMap, origin: &str) -> bool {
    let own = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|host| format!("https://{host}"));
    if own.as_deref() == Some(origin) {
        return true;
    }
    let configured = h
        .config
        .read()
        .unwrap()
        .get("csrf_allowed_origins", "[]")
        .to_owned();
    butterpollo_config::config::Config::parse(&format!("csrf_allowed_origins = {configured}\n"))
        .map(|c| c.list("csrf_allowed_origins"))
        .unwrap_or_default()
        .iter()
        .any(|allowed| allowed == origin)
}

fn issued(
    h: &Shared,
    username: String,
    remember_me: bool,
    headers: &HeaderMap,
    connection: &Connection,
    previous: Option<web_sessions::WebSession>,
) -> Response {
    let username = h
        .credentials
        .read()
        .unwrap()
        .as_ref()
        .filter(|c| c.username.eq_ignore_ascii_case(&username))
        .map(|c| c.username.clone())
        .unwrap_or(username);
    let ttl = h
        .config
        .read()
        .unwrap()
        .integer("session_token_ttl_seconds", 7200)
        .clamp(60, 604800) as u64;
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(512)
        .collect();
    let (access, refresh, csrf, refresh_ttl) = match h.new_web_session(
        username,
        remember_me,
        user_agent,
        connection.peer.ip().to_string(),
        previous,
    ) {
        Ok(tokens) => tokens,
        Err(e) => {
            if e.is::<web_sessions::Rotated>() {
                return error(StatusCode::UNAUTHORIZED, "refresh token expired");
            }
            tracing::error!(error=%e, "browser session could not be persisted");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "browser session could not be saved",
            );
        }
    };
    let ttl = ttl.min(refresh_ttl);
    let mut r = response(
        StatusCode::OK,
        json!({"status":true,"access_token":access,"refresh_token":refresh,"csrf_token":csrf,"expires_in":ttl,"refresh_expires_in":refresh_ttl,"remember_me":remember_me,"redirect":"/"}),
    );
    for (name, value, lifetime) in [
        ("__Host-apollo_session", access, ttl),
        ("__Host-apollo_refresh", refresh, refresh_ttl),
    ] {
        let expiry = if remember_me {
            format!("; Max-Age={lifetime}")
        } else {
            String::new()
        };
        r.headers_mut().append(
            header::SET_COOKIE,
            format!("{name}={value}; Path=/; HttpOnly; SameSite=Strict; Secure{expiry}")
                .parse()
                .unwrap(),
        );
    }
    r
}

pub(crate) fn refresh_browser(
    h: &Shared,
    headers: &HeaderMap,
    connection: &Connection,
) -> Option<Response> {
    let token = cookie(headers, "__Host-apollo_refresh")?;
    let previous = h
        .web_sessions
        .lock()
        .unwrap()
        .values()
        .find(|s| {
            crypto::matches_hash(token.as_bytes(), &s.refresh) && s.refresh_expires > Instant::now()
        })
        .cloned()?;
    Some(issued(
        h,
        previous.username.clone(),
        previous.remember_me,
        headers,
        connection,
        Some(previous),
    ))
}

pub(super) fn status(h: &Shared, headers: &HeaderMap, connection: &Connection) -> Response {
    let configured = h.credentials.read().unwrap().is_some();
    let authenticated = authenticated(h, headers, Some(connection.peer.ip()));
    // The web app asks this on load. A remembered browser whose access
    // cookie lapsed (after session_token_ttl_seconds) is renewed here, as
    // the console page does, instead of being sent to the login page.
    if configured
        && !authenticated
        && let Some(issued) = refresh_browser(h, headers, connection)
        && issued.status().is_success()
    {
        let mut result = Json(json!({"authenticated":true,"credentials_configured":true,"login_required":false,"status":true})).into_response();
        for value in issued.headers().get_all(header::SET_COOKIE) {
            result
                .headers_mut()
                .append(header::SET_COOKIE, value.clone());
        }
        return result;
    }
    let mut status = json!({"authenticated":authenticated,"credentials_configured":configured,"login_required":configured&&!authenticated,"status":true});
    // The sign-in page on this PC shows how to set a new sign-in: an
    // administrator runs this program with --creds.
    if configured
        && !authenticated
        && butterpollo_stream::network::from_this_pc(connection.peer)
        && let Ok(program) = std::env::current_exe()
    {
        status["creds_program"] = program.display().to_string().into();
    }
    Json(status).into_response()
}

pub(super) fn login(
    h: &Shared,
    method: &Method,
    headers: &HeaderMap,
    connection: &Connection,
    data: &Value,
) -> Response {
    if *method != Method::POST {
        return error(StatusCode::METHOD_NOT_ALLOWED, "POST required");
    }
    let now = Instant::now();
    if !SIGN_INS.lock().unwrap().allowed(connection.peer.ip(), now) {
        return error(StatusCode::TOO_MANY_REQUESTS, "try again in one minute");
    }
    let text = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("");
    let username = text("username");
    let password = text("password");
    let (verified, upgrade_user) = {
        let guard = h.credentials.read().unwrap();
        match guard.as_ref() {
            Some(c) if c.verifies(username, password) => {
                (true, c.needs_rehash().then(|| c.username.clone()))
            }
            _ => (false, None),
        }
    };
    if verified {
        SIGN_INS.lock().unwrap().succeeded(connection.peer.ip());
        if let Some(stored_user) = upgrade_user {
            rehash_if_needed(h, stored_user, password);
        }
        let remember = data.get("remember_me").is_some_and(|v| {
            v.as_bool()
                .unwrap_or_else(|| matches!(v.as_str(), Some("true" | "1" | "on")))
        });
        return issued(h, username.into(), remember, headers, connection, None);
    }
    SIGN_INS.lock().unwrap().failed(connection.peer.ip(), now);
    error(StatusCode::UNAUTHORIZED, "invalid username or password")
}

pub(super) fn refresh(
    h: &Shared,
    method: &Method,
    headers: &HeaderMap,
    connection: &Connection,
    data: &Value,
) -> Response {
    if *method != Method::POST {
        return error(StatusCode::METHOD_NOT_ALLOWED, "POST required");
    }
    let text = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("");
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Refresh "))
        .map(str::to_owned)
        .or_else(|| cookie(headers, "__Host-apollo_refresh"))
        .unwrap_or_else(|| text("refresh_token").into());
    let previous = {
        let sessions = h.web_sessions.lock().unwrap();
        sessions
            .iter()
            .find(|(_, s)| {
                crypto::matches_hash(token.as_bytes(), &s.refresh)
                    && s.refresh_expires > Instant::now()
            })
            .map(|(_, s)| s.clone())
    };
    match previous {
        Some(s) => issued(
            h,
            s.username.clone(),
            s.remember_me,
            headers,
            connection,
            Some(s),
        ),
        None => error(StatusCode::UNAUTHORIZED, "refresh token expired"),
    }
}

pub(super) fn logout(h: &Shared, headers: &HeaderMap) -> Response {
    if let Some(token) = access(headers) {
        let mut sessions = h.web_sessions.lock().unwrap();
        let mut next = sessions.clone();
        if let Some(key) = web_sessions::resolve_hash(&next, &token) {
            next.remove(&key);
        }
        if let Err(e) = h.save_web_sessions(&next) {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
        *sessions = next;
    }
    let mut r = Json(json!({"status":true})).into_response();
    for name in ["__Host-apollo_session", "__Host-apollo_refresh"] {
        r.headers_mut().append(
            header::SET_COOKIE,
            format!("{name}=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0")
                .parse()
                .unwrap(),
        );
    }
    r
}

pub(super) fn csrf_token(h: &Shared, method: &Method, headers: &HeaderMap) -> Response {
    if *method != Method::GET {
        return error(StatusCode::METHOD_NOT_ALLOWED, "GET required");
    }
    let token = access(headers);
    let csrf = token
        .and_then(|t| {
            web_sessions::find(&h.web_sessions.lock().unwrap(), &t).map(|s| s.csrf.clone())
        })
        .unwrap_or_default();
    if !csrf.is_empty() {
        return Json(json!({"status":true,"csrf_token":csrf,"token":csrf})).into_response();
    }
    let csrf = hex::encode(crypto::random::<32>());
    let mut result = Json(json!({"status":true,"csrf_token":csrf,"token":csrf})).into_response();
    result.headers_mut().insert(
        header::SET_COOKIE,
        format!("__Host-apollo_anon_csrf={csrf}; Path=/; HttpOnly; SameSite=Strict; Secure; Max-Age=3600")
            .parse()
            .unwrap(),
    );
    result
}

pub(super) fn list_sessions(h: &Shared, headers: &HeaderMap) -> Value {
    let sessions = h.web_sessions.lock().unwrap();
    let current = web_sessions::resolve_hash(&sessions, &access(headers).unwrap_or_default());
    let sessions: Vec<_> = sessions
        .iter()
        .filter(|(_, s)| s.refresh_expires > Instant::now())
        .map(|(token, s)| {
            let mut row = web_sessions::record(token, s);
            let object = row.as_object_mut().unwrap();
            object.remove("refresh_token_hash");
            object.remove("rotation_id");
            object.remove(web_sessions::SIGN_IN_ID);
            object.remove("hash");
            object.insert("id".into(), json!(token));
            object.insert(
                "current".into(),
                json!(
                    current
                        .as_ref()
                        .is_some_and(|current| crypto::equal(token.as_bytes(), current.as_bytes()))
                ),
            );
            row
        })
        .collect();
    json!({"status":true,"sessions":sessions})
}

pub(super) fn delete_session(h: &Shared, hash: &str) -> anyhow::Result<Value> {
    let mut sessions = h.web_sessions.lock().unwrap();
    let mut next = sessions.clone();
    next.retain(|token, _| !token.eq_ignore_ascii_case(hash));
    h.save_web_sessions(&next)?;
    *sessions = next;
    Ok(json!({"status":true,"deleted":true}))
}

pub(super) fn change_password(h: &Shared, data: &Value) -> anyhow::Result<Value> {
    let text = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("");
    if let Some(c) = h.credentials.read().unwrap().as_ref()
        && !c.verifies(text("currentUsername"), text("currentPassword"))
    {
        anyhow::bail!("current credentials do not match");
    }
    let password = text("newPassword");
    if password != text("confirmNewPassword") {
        anyhow::bail!("password confirmation does not match");
    }
    let c = Credentials::new(text("newUsername").into(), password)?;
    // Invalidate durable sessions and API tokens before changing
    // the password. A failed credential write may require another
    // login; it cannot resurrect what the previous password let in.
    let mut sessions = h.web_sessions.lock().unwrap();
    h.revoke_sign_ins()?;
    sessions.clear();
    h.save_credentials(&c)?;
    *h.credentials.write().unwrap() = Some(c);
    Ok(json!({"status":true}))
}
