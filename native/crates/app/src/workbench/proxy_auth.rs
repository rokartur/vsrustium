//! The proxy requests of MainThreadWorkspace: `$loadCertificates` (requestService.ts `loadCertificates`, the macOS
//! keychain certificates proxyResolver.ts adds to the host's TLS connections) and `$lookupAuthorization`
//! (auth.ts `ProxyAuthService`: credentials for a proxy that answered 407).

use std::collections::HashMap;

use base64::Engine;
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;

/// `ProxyAuthService`'s memory between requests, per auth info hash.
#[derive(Default)]
pub struct ProxyAuth {
    /// The configured credentials last answered and whether the proxy rejected them (`configuredCredentialsStates`).
    configured: HashMap<String, (String, bool)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl Workbench {
    /// `$loadCertificates`: read off the UI thread, answered when `security` is done.
    pub(crate) fn load_certificates(&self, cx: &mut gpui::Context<Self>) -> Outcome {
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.background_executor()
            .spawn(async move {
                let _ = sender.send(Ok(json!(read_mac_ca_certificates())));
            })
            .detach();
        Outcome::Pending(receiver)
    }

    /// `$lookupAuthorization(authInfo)`.
    pub(crate) fn lookup_authorization(&mut self, auth_info: &Value) -> Outcome {
        let proxy = self.setting("http.proxy").as_str().unwrap_or_default().trim().to_owned();
        let credentials = resolve_proxy_credentials(&mut self.proxy_auth, auth_info, self.skip_dialogs, &proxy, &|key| std::env::var(key).ok());
        Outcome::Ready(Ok(credentials.map_or(Value::Null, |credentials| json!({ "username": credentials.username, "password": credentials.password }))))
    }
}

/// `onLogin` and `doResolveProxyCredentials`: extension tests read the credentials from the realm; otherwise the
/// credentials of `http.proxy` or the proxy environment variables until the proxy rejects them.
// shortcut: no proxy login dialog (`vscode:openProxyAuthenticationDialog`), so no session or stored credentials: a
// proxy that needs credentials the settings do not hold fails its requests; port the dialog with its Remember checkbox.
pub fn resolve_proxy_credentials(state: &mut ProxyAuth, auth_info: &Value, extension_tests: bool, proxy_setting: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<Credentials> {
    if auth_info["isProxy"] != true {
        return None;
    }
    if extension_tests {
        let realm = base64::engine::general_purpose::STANDARD.decode(auth_info["realm"].as_str()?).ok()?;
        let realm = String::from_utf8(realm).ok().filter(|realm| realm.starts_with('{'))?;
        let parsed: Value = serde_json::from_str(&realm).ok()?;
        return Some(Credentials { username: parsed["username"].as_str()?.to_owned(), password: parsed["password"].as_str()?.to_owned() });
    }
    // `hash({ scheme, host, port })`: any stable key of the three serves.
    let key = json!([auth_info["scheme"], auth_info["host"], auth_info["port"]]).to_string();
    let configured = configured_proxy_credentials(proxy_setting, env)?;
    let credentials_key = json!([configured.username, configured.password]).to_string();
    match state.configured.get_mut(&key) {
        Some((last, rejected)) if *last == credentials_key => {
            if *rejected {
                return None;
            }
            if auth_info["attempt"].as_u64().unwrap_or(1) == 1 {
                return Some(configured);
            }
            *rejected = true;
            None
        }
        _ => {
            state.configured.insert(key, (credentials_key, false));
            Some(configured)
        }
    }
}

/// `readConfiguredProxyCredentials`: the user info of `http.proxy`, else of the https or http proxy variable.
fn configured_proxy_credentials(proxy_setting: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<Credentials> {
    let proxy = match proxy_setting.is_empty() {
        false => proxy_setting.to_owned(),
        true => ["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY"].iter().find_map(|key| env(key).filter(|value| !value.is_empty())).unwrap_or_default().trim().to_owned(),
    };
    let authority = proxy.split_once("://").map_or(proxy.as_str(), |(_, rest)| rest);
    let authority = authority.split(['/', '?', '#']).next().unwrap_or_default();
    let (userinfo, _) = authority.split_once('@')?;
    Some(match userinfo.split_once(':') {
        Some((username, password)) => Credentials { username: username.into(), password: password.into() },
        None => Credentials { username: userinfo.into(), password: String::new() },
    })
}

/// @vscode/proxy-agent `readMacCaCertificates`: every certificate `security find-certificate -a -p` prints, once.
// shortcut: upstream's default (`http.systemCertificatesNode`) asks Node's `tls.getCACertificates('system')` and drops
// expired certificates; the keychain dump also holds non-root and expired certificates, harmless as extra CAs.
fn read_mac_ca_certificates() -> Vec<String> {
    let Ok(output) = std::process::Command::new("/usr/bin/security").args(["find-certificate", "-a", "-p"]).output() else { return Vec::new() };
    split_pem_certificates(&String::from_utf8_lossy(&output.stdout))
}

/// `stdout.split(/(?=-----BEGIN CERTIFICATE-----)/g)` without empty parts, duplicates dropped in first-seen order.
pub fn split_pem_certificates(text: &str) -> Vec<String> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    let mut certificates: Vec<String> = Vec::new();
    let starts: Vec<usize> = text.match_indices(BEGIN).map(|(index, _)| index).collect();
    let mut bounds = vec![0];
    bounds.extend(starts.iter().copied().filter(|start| *start > 0));
    bounds.push(text.len());
    for pair in bounds.windows(2) {
        let part = &text[pair[0]..pair[1]];
        if !part.is_empty() && !certificates.iter().any(|known| known == part) {
            certificates.push(part.to_owned());
        }
    }
    certificates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(realm: &str, attempt: u64) -> Value {
        json!({ "isProxy": true, "scheme": "basic", "host": "127.0.0.1", "port": 8080, "realm": realm, "attempt": attempt })
    }

    #[test]
    fn extension_tests_read_the_credentials_from_the_realm() {
        let realm = base64::engine::general_purpose::STANDARD.encode(r#"{"username":"testuser","password":"testpassword"}"#);
        let credentials = resolve_proxy_credentials(&mut ProxyAuth::default(), &info(&realm, 1), true, "", &|_| None);
        assert_eq!(credentials, Some(Credentials { username: "testuser".into(), password: "testpassword".into() }));
        assert_eq!(resolve_proxy_credentials(&mut ProxyAuth::default(), &info("", 1), true, "", &|_| None), None);
        let not_proxy = json!({ "isProxy": false, "realm": realm });
        assert_eq!(resolve_proxy_credentials(&mut ProxyAuth::default(), &not_proxy, true, "", &|_| None), None);
    }

    #[test]
    fn configured_credentials_answer_until_the_proxy_rejects_them() {
        let mut state = ProxyAuth::default();
        let expected = Some(Credentials { username: "ann".into(), password: "p:w".into() });
        let proxy = "http://ann:p:w@proxy:3128";
        assert_eq!(resolve_proxy_credentials(&mut state, &info("", 1), false, proxy, &|_| None), expected);
        assert_eq!(resolve_proxy_credentials(&mut state, &info("", 1), false, proxy, &|_| None), expected);
        assert_eq!(resolve_proxy_credentials(&mut state, &info("", 2), false, proxy, &|_| None), None, "rejected: no dialog to ask");
        assert_eq!(resolve_proxy_credentials(&mut state, &info("", 1), false, proxy, &|_| None), None);
        let changed = "http://ann:new@proxy:3128";
        assert_eq!(resolve_proxy_credentials(&mut state, &info("", 2), false, changed, &|_| None).map(|credentials| credentials.password), Some("new".into()));
        let env = |key: &str| (key == "HTTP_PROXY").then(|| "http://bob@proxy:3128".to_owned());
        assert_eq!(configured_proxy_credentials("", &env), Some(Credentials { username: "bob".into(), password: String::new() }));
        assert_eq!(configured_proxy_credentials("http://proxy:3128", &env), None);
    }

    #[test]
    fn the_keychain_dump_splits_into_unique_certificates() {
        let a = "-----BEGIN CERTIFICATE-----\nAAA\n-----END CERTIFICATE-----\n";
        let b = "-----BEGIN CERTIFICATE-----\nBBB\n-----END CERTIFICATE-----\n";
        assert_eq!(split_pem_certificates(&format!("{a}{b}{a}")), [a, b]);
        assert!(split_pem_certificates("").is_empty());
    }
}
