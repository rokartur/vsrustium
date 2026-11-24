//! mainThreadSecretState.ts over the macOS Keychain. Upstream encrypts each secret with Electron safeStorage,
//! whose key lives in the Keychain, and keeps it in application storage under `secret://<key>`. The port stores
//! each secret as a generic password item instead: service `Code - OSS Secret Storage`, account the same
//! `{"extensionId","key"}` key, so the Keychain encrypts it and no key handling is needed here.

use std::collections::BTreeMap;

use security_framework::base::Error as KeychainError;
use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
use security_framework::os::macos::keychain::SecKeychain;
use serde_json::{Value, json};

use crate::rpc::{Arg, Message, Reply};

const SERVICE: &str = "Code - OSS Secret Storage";
/// errSecItemNotFound.
const ITEM_NOT_FOUND: i32 = -25300;

pub struct MainThreadSecretState {
    secrets: Secrets,
}

enum Secrets {
    Keychain(SecKeychain),
    /// Extension tests: application storage is in memory, and so are the secrets kept in it.
    Memory(BTreeMap<String, String>),
}

impl MainThreadSecretState {
    pub fn keychain() -> Result<Self, KeychainError> {
        Ok(MainThreadSecretState { secrets: Secrets::Keychain(SecKeychain::default()?) })
    }

    pub fn in_memory() -> Self {
        MainThreadSecretState { secrets: Secrets::Memory(BTreeMap::new()) }
    }

    /// Request `req` from the host: the `$onDidChangePassword` argument to send first if a secret changed,
    /// and the reply. A Keychain failure rejects with its message.
    pub fn serve(&mut self, req: u32, method: &str, args: &[Arg]) -> (Option<Value>, Message) {
        match self.handle(method, args) {
            Ok((change, value)) => (change, Message::ReplyOk { req, value }),
            Err(error) => {
                let error = json!({ "$isError": true, "name": "Error", "message": error.to_string(), "stack": "" });
                (None, Message::ReplyErr { req, error: Some(error) })
            }
        }
    }

    fn handle(&mut self, method: &str, args: &[Arg]) -> Result<(Option<Value>, Reply), KeychainError> {
        let arg = |index: usize| match args.get(index) {
            Some(Arg::Json(Value::String(text))) => text.as_str(),
            _ => "",
        };
        let extension_id = arg(0);
        let key = json!({ "extensionId": extension_id, "key": arg(1) });
        let account = key.to_string();
        match method {
            "$getPassword" => Ok((None, self.get(&account)?.map_or(Reply::Empty, |secret| Reply::Json(Value::String(secret))))),
            "$setPassword" => {
                let value = arg(2);
                if self.get(&account)?.as_deref() == Some(value) {
                    return Ok((None, Reply::Empty));
                }
                self.set(&account, value)?;
                Ok((Some(key), Reply::Empty))
            }
            "$deletePassword" => {
                let deleted = self.delete(&account)?;
                Ok((deleted.then_some(key), Reply::Empty))
            }
            "$getKeys" => {
                let mut keys = Vec::new();
                for account in self.accounts()? {
                    let Ok(parsed) = serde_json::from_str::<Value>(&account) else { continue };
                    if parsed["extensionId"] == extension_id
                        && let Some(key) = parsed["key"].as_str()
                    {
                        keys.push(Value::String(key.to_owned()));
                    }
                }
                Ok((None, Reply::Json(Value::Array(keys))))
            }
            _ => Ok((None, Reply::Empty)),
        }
    }

    fn get(&self, account: &str) -> Result<Option<String>, KeychainError> {
        match &self.secrets {
            Secrets::Memory(secrets) => Ok(secrets.get(account).cloned()),
            Secrets::Keychain(keychain) => match keychain.find_generic_password(SERVICE, account) {
                Ok((password, _)) => Ok(Some(String::from_utf8_lossy(&password).into_owned())),
                Err(error) if error.code() == ITEM_NOT_FOUND => Ok(None),
                Err(error) => Err(error),
            },
        }
    }

    fn set(&mut self, account: &str, value: &str) -> Result<(), KeychainError> {
        match &mut self.secrets {
            Secrets::Memory(secrets) => {
                secrets.insert(account.to_owned(), value.to_owned());
                Ok(())
            }
            Secrets::Keychain(keychain) => keychain.set_generic_password(SERVICE, account, value.as_bytes()),
        }
    }

    /// Whether a secret was there to delete.
    fn delete(&mut self, account: &str) -> Result<bool, KeychainError> {
        match &mut self.secrets {
            Secrets::Memory(secrets) => Ok(secrets.remove(account).is_some()),
            Secrets::Keychain(keychain) => match keychain.find_generic_password(SERVICE, account) {
                Ok((_, item)) => {
                    item.delete();
                    Ok(true)
                }
                Err(error) if error.code() == ITEM_NOT_FOUND => Ok(false),
                Err(error) => Err(error),
            },
        }
    }

    /// The account of every secret, for `secretStorageService.keys()`.
    fn accounts(&self) -> Result<Vec<String>, KeychainError> {
        let keychain = match &self.secrets {
            Secrets::Memory(secrets) => return Ok(secrets.keys().cloned().collect()),
            Secrets::Keychain(keychain) => keychain,
        };
        let found = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .keychains(std::slice::from_ref(keychain))
            .service(SERVICE)
            .load_attributes(true)
            .limit(Limit::All)
            .search();
        let results = match found {
            Ok(results) => results,
            Err(error) if error.code() == ITEM_NOT_FOUND => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        Ok(results.iter().filter_map(|result| result.simplify_dict()?.remove("acct")).collect())
    }
}

/// The secret storage flow of extHostSecretState and mainThreadSecretState on a throwaway keychain file, so the
/// login keychain is never touched.
#[cfg(test)]
mod tests {
    use security_framework::os::macos::keychain::CreateOptions;

    use super::*;

    fn call(state: &mut MainThreadSecretState, method: &str, args: &[&str]) -> (Option<Value>, Value) {
        let args: Vec<Arg> = args.iter().map(|arg| Arg::Json(json!(arg))).collect();
        match state.serve(1, method, &args) {
            (change, Message::ReplyOk { value: Reply::Empty, .. }) => (change, Value::Null),
            (change, Message::ReplyOk { value: Reply::Json(value), .. }) => (change, value),
            (_, reply) => panic!("{method} failed: {reply:?}"),
        }
    }

    fn check_flow(state: &mut MainThreadSecretState) {
        assert_eq!(call(state, "$getPassword", &["pub.ext", "token"]), (None, Value::Null));
        let changed = Some(json!({ "extensionId": "pub.ext", "key": "token" }));
        assert_eq!(call(state, "$setPassword", &["pub.ext", "token", "s3cret"]), (changed.clone(), Value::Null));
        assert_eq!(call(state, "$setPassword", &["pub.ext", "token", "s3cret"]), (None, Value::Null), "unchanged value fires no event");
        call(state, "$setPassword", &["pub.ext", "other", "x"]);
        call(state, "$setPassword", &["pub.another", "token", "y"]);
        assert_eq!(call(state, "$getPassword", &["pub.ext", "token"]), (None, json!("s3cret")));
        let (_, keys) = call(state, "$getKeys", &["pub.ext"]);
        let mut keys: Vec<&str> = keys.as_array().unwrap().iter().map(|key| key.as_str().unwrap()).collect();
        keys.sort();
        assert_eq!(keys, ["other", "token"]);
        assert_eq!(call(state, "$deletePassword", &["pub.ext", "token"]), (changed, Value::Null));
        assert_eq!(call(state, "$deletePassword", &["pub.ext", "token"]), (None, Value::Null));
        assert_eq!(call(state, "$getPassword", &["pub.ext", "token"]), (None, Value::Null));
        assert_eq!(call(state, "$getPassword", &["pub.another", "token"]), (None, json!("y")));
    }

    #[test]
    fn secrets_in_a_keychain() {
        let dir = std::env::temp_dir().join(format!("code-gpui-keychain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let keychain = CreateOptions::new().password("test").create(dir.join("test.keychain")).unwrap();
        check_flow(&mut MainThreadSecretState { secrets: Secrets::Keychain(keychain) });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn secrets_in_memory() {
        check_flow(&mut MainThreadSecretState::in_memory());
    }
}
