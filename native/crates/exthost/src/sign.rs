//! platform/sign and extensionSignatureVerificationService.ts for the signatures Open VSX serves.
//!
//! Upstream verifies with `@vscode/vsce-sign`: a PKCS#7 `.signature.p7s` chained to Microsoft's certificates.
//! Open VSX ships an empty `.signature.p7s` and signs instead with Ed25519: `.signature.sig` is the signature of
//! the VSIX bytes by the key of the version's `Microsoft.VisualStudio.Services.PublicKey` asset, and
//! `.signature.manifest` lists the package and every entry with its size and SHA-256, as vsce-sign's manifest does.
//! This checks all three and answers upstream's `ExtensionSignatureVerificationCode` names.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::zip::Archive;

/// `ExtensionSignatureVerificationCode`, the codes this verifier can answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationCode {
    NotSigned,
    Success,
    PackageIsInvalidZip,
    SignatureArchiveIsInvalidZip,
    SignatureManifestIsMissing,
    SignatureManifestIsUnreadable,
    SignatureIsMissing,
    SignatureIsUnreadable,
    CertificateIsUnreadable,
    PackageIntegrityCheckFailed,
    SignatureManifestIsInvalid,
    EntryIsMissing,
    EntryIsTampered,
    SignatureIsInvalid,
    SignatureArchiveHasTooManyEntries,
}

impl VerificationCode {
    pub fn name(self) -> &'static str {
        match self {
            Self::NotSigned => "NotSigned",
            Self::Success => "Success",
            Self::PackageIsInvalidZip => "PackageIsInvalidZip",
            Self::SignatureArchiveIsInvalidZip => "SignatureArchiveIsInvalidZip",
            Self::SignatureManifestIsMissing => "SignatureManifestIsMissing",
            Self::SignatureManifestIsUnreadable => "SignatureManifestIsUnreadable",
            Self::SignatureIsMissing => "SignatureIsMissing",
            Self::SignatureIsUnreadable => "SignatureIsUnreadable",
            Self::CertificateIsUnreadable => "CertificateIsUnreadable",
            Self::PackageIntegrityCheckFailed => "PackageIntegrityCheckFailed",
            Self::SignatureManifestIsInvalid => "SignatureManifestIsInvalid",
            Self::EntryIsMissing => "EntryIsMissing",
            Self::EntryIsTampered => "EntryIsTampered",
            Self::SignatureIsInvalid => "SignatureIsInvalid",
            Self::SignatureArchiveHasTooManyEntries => "SignatureArchiveHasTooManyEntries",
        }
    }
}

/// vsce-sign refuses signature archives with more than the three signature files.
const MAX_SIGNATURE_ARCHIVE_ENTRIES: usize = 3;

/// Verifies `vsix` against the signature archive and the PEM public key.
pub fn verify(vsix: &[u8], signature_archive: &[u8], public_key_pem: &str) -> VerificationCode {
    match check(vsix, signature_archive, public_key_pem) {
        Ok(()) => VerificationCode::Success,
        Err(code) => code,
    }
}

fn check(vsix: &[u8], signature_archive: &[u8], public_key_pem: &str) -> Result<(), VerificationCode> {
    use VerificationCode::*;
    let archive = Archive::open(signature_archive.to_vec()).map_err(|_| SignatureArchiveIsInvalidZip)?;
    if archive.entries.len() > MAX_SIGNATURE_ARCHIVE_ENTRIES {
        return Err(SignatureArchiveHasTooManyEntries);
    }
    let manifest = archive.entry(".signature.manifest").ok_or(SignatureManifestIsMissing)?;
    let manifest = archive.read(manifest).map_err(|_| SignatureManifestIsUnreadable)?;
    let signature = archive.entry(".signature.sig").ok_or(SignatureIsMissing)?;
    let signature = archive.read(signature).map_err(|_| SignatureIsUnreadable)?;
    let signature = Signature::from_slice(&signature).map_err(|_| SignatureIsUnreadable)?;
    let key = public_key(public_key_pem).ok_or(CertificateIsUnreadable)?;
    key.verify(vsix, &signature).map_err(|_| SignatureIsInvalid)?;

    let manifest: Value = serde_json::from_slice(&manifest).map_err(|_| SignatureManifestIsUnreadable)?;
    let package = &manifest["package"];
    if package["size"].as_u64() != Some(vsix.len() as u64) || package["digests"]["sha256"].as_str() != Some(&sha256_base64(vsix)) {
        return Err(PackageIntegrityCheckFailed);
    }
    let entries = manifest["entries"].as_object().ok_or(SignatureManifestIsInvalid)?;
    let package = Archive::open(vsix.to_vec()).map_err(|_| PackageIsInvalidZip)?;
    if package.entries.len() != entries.len() {
        return Err(SignatureManifestIsInvalid);
    }
    for (encoded_name, expected) in entries {
        let name = STANDARD.decode(encoded_name).ok().and_then(|name| String::from_utf8(name).ok()).ok_or(SignatureManifestIsInvalid)?;
        let entry = package.entry(&name).ok_or(EntryIsMissing)?;
        let contents = package.read(entry).map_err(|_| EntryIsTampered)?;
        if expected["size"].as_u64() != Some(contents.len() as u64) || expected["digests"]["sha256"].as_str() != Some(&sha256_base64(&contents)) {
            return Err(EntryIsTampered);
        }
    }
    Ok(())
}

/// An Ed25519 `SubjectPublicKeyInfo` in PEM: a fixed 12-byte DER prefix, then the 32-byte key.
fn public_key(pem: &str) -> Option<VerifyingKey> {
    const ED25519_SPKI_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
    let body: String = pem.lines().filter(|line| !line.starts_with("-----")).collect();
    let der = STANDARD.decode(body.trim()).ok()?;
    let key = der.strip_prefix(&ED25519_SPKI_PREFIX[..])?;
    VerifyingKey::from_bytes(key.try_into().ok()?).ok()
}

fn sha256_base64(bytes: &[u8]) -> String {
    STANDARD.encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    use super::*;
    use crate::zip::write_stored;

    struct Signed {
        vsix: Vec<u8>,
        archive: Vec<u8>,
        pem: String,
    }

    fn pem(key: &SigningKey) -> String {
        let mut der = vec![0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
        der.extend_from_slice(key.verifying_key().as_bytes());
        format!("-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n", STANDARD.encode(der))
    }

    fn manifest(vsix: &[u8], files: &[(&str, &[u8])]) -> Value {
        let mut entries = serde_json::Map::new();
        for (name, contents) in files {
            entries.insert(STANDARD.encode(name), json!({ "size": contents.len(), "digests": { "sha256": sha256_base64(contents) } }));
        }
        json!({ "package": { "size": vsix.len(), "digests": { "sha256": sha256_base64(vsix) } }, "entries": entries })
    }

    fn sign(files: &[(&str, &[u8])]) -> Signed {
        let key = SigningKey::from_bytes(&[7; 32]);
        let vsix = write_stored(files);
        let manifest = serde_json::to_vec(&manifest(&vsix, files)).unwrap();
        let signature = key.sign(&vsix).to_bytes();
        let archive = write_stored(&[(".signature.sig", &signature), (".signature.manifest", &manifest), (".signature.p7s", b"")]);
        Signed { vsix, archive, pem: pem(&key) }
    }

    const FILES: &[(&str, &[u8])] = &[("extension.vsixmanifest", b"<xml/>"), ("extension/package.json", b"{\"name\":\"a\"}")];

    #[test]
    fn a_signed_package_verifies() {
        let signed = sign(FILES);
        assert_eq!(verify(&signed.vsix, &signed.archive, &signed.pem), VerificationCode::Success);
    }

    #[test]
    fn another_key_is_an_invalid_signature() {
        let signed = sign(FILES);
        let other = pem(&SigningKey::from_bytes(&[8; 32]));
        assert_eq!(verify(&signed.vsix, &signed.archive, &other), VerificationCode::SignatureIsInvalid);
    }

    #[test]
    fn a_changed_package_is_an_invalid_signature() {
        let signed = sign(FILES);
        let tampered = write_stored(&[("extension.vsixmanifest", b"<xml/>"), ("extension/package.json", b"{\"name\":\"b\"}")]);
        assert_eq!(verify(&tampered, &signed.archive, &signed.pem), VerificationCode::SignatureIsInvalid);
    }

    #[test]
    fn a_manifest_that_does_not_match_the_signed_package() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let vsix = write_stored(FILES);
        let signature = key.sign(&vsix).to_bytes();
        let archive = |manifest: &Value| write_stored(&[(".signature.sig", &signature), (".signature.manifest", &serde_json::to_vec(manifest).unwrap())]);

        let mut wrong_package = manifest(&vsix, FILES);
        wrong_package["package"]["size"] = json!(1);
        assert_eq!(verify(&vsix, &archive(&wrong_package), &pem(&key)), VerificationCode::PackageIntegrityCheckFailed);

        let mut tampered = manifest(&vsix, FILES);
        tampered["entries"][STANDARD.encode("extension/package.json")]["digests"]["sha256"] = json!(sha256_base64(b"other"));
        assert_eq!(verify(&vsix, &archive(&tampered), &pem(&key)), VerificationCode::EntryIsTampered);

        let missing = manifest(&vsix, &[("extension.vsixmanifest", b"<xml/>"), ("extension/other.js", b"")]);
        assert_eq!(verify(&vsix, &archive(&missing), &pem(&key)), VerificationCode::EntryIsMissing);

        let unlisted = manifest(&vsix, &FILES[..1]);
        assert_eq!(verify(&vsix, &archive(&unlisted), &pem(&key)), VerificationCode::SignatureManifestIsInvalid);
    }

    #[test]
    fn broken_signature_archives() {
        let signed = sign(FILES);
        assert_eq!(verify(&signed.vsix, b"not a zip", &signed.pem), VerificationCode::SignatureArchiveIsInvalidZip);
        let no_signature = write_stored(&[(".signature.manifest", b"{}")]);
        assert_eq!(verify(&signed.vsix, &no_signature, &signed.pem), VerificationCode::SignatureIsMissing);
        let no_manifest = write_stored(&[(".signature.sig", &[0; 64])]);
        assert_eq!(verify(&signed.vsix, &no_manifest, &signed.pem), VerificationCode::SignatureManifestIsMissing);
        let short = write_stored(&[(".signature.sig", &[0; 10]), (".signature.manifest", b"{}")]);
        assert_eq!(verify(&signed.vsix, &short, &signed.pem), VerificationCode::SignatureIsUnreadable);
        assert_eq!(verify(&signed.vsix, &signed.archive, "-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----"), VerificationCode::CertificateIsUnreadable);
        let four = write_stored(&[("a", b""), ("b", b""), ("c", b""), ("d", b"")]);
        assert_eq!(verify(&signed.vsix, &four, &signed.pem), VerificationCode::SignatureArchiveHasTooManyEntries);
    }
}
