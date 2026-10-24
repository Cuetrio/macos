use std::fmt;

use crate::update_manifest::{UpdateManifest, UpdateRelease, MANIFEST_SCHEMA_VERSION};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::digest::{digest, SHA256};
use ring::signature::{UnparsedPublicKey, ED25519};

pub(crate) const SIGNATURE_PAYLOAD_VERSION: u8 = 1;
pub(crate) const SIGNATURE_PAYLOAD_DOMAIN_BYTES: &[u8] =
    b"com.cornellmethod.notebook/desktop-update-signature/ed25519\0";
pub(crate) const TRUSTED_KEY_ID_PREFIX: &str = "cmn-ed25519-v1-";

const SIGNED_IDENTITY_DOMAIN_BYTES: &[u8] =
    b"com.cornellmethod.notebook/desktop-update-signed-identity/sha256\0";

const TRUSTED_PUBLIC_KEY_BYTES: usize = 32;
const ED25519_SIGNATURE_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrustedKeyStatus {
    Current,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "embedded key table constructs only Current; tests require Next key acceptance"
        )
    )]
    Next,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "embedded key table constructs only Current; tests require distinct Retired key rejection"
        )
    )]
    Retired,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "embedded key table constructs only Current; tests require distinct Revoked key rejection"
        )
    )]
    Revoked,
}

#[derive(Clone)]
pub(crate) struct TrustedKeyEntry {
    key_id: String,
    public_key: [u8; TRUSTED_PUBLIC_KEY_BYTES],
    status: TrustedKeyStatus,
}

#[derive(Clone, Copy)]
struct EmbeddedTrustedKeySpec {
    key_id: &'static str,
    public_key: [u8; TRUSTED_PUBLIC_KEY_BYTES],
    status: TrustedKeyStatus,
}

// The only non-test source for trusted keys is this compile-time table.
const EMBEDDED_TRUSTED_KEY_TABLE: &[EmbeddedTrustedKeySpec] = &[EmbeddedTrustedKeySpec {
    key_id: "cmn-ed25519-v1-381374c2723e7e3624ed21bd2836992ae9266ef78dfeb9ac21b33a08e8632f54",
    public_key: [
        0xf1, 0xca, 0xe2, 0x84, 0x7e, 0x46, 0x9f, 0x73, 0x66, 0x23, 0x0a, 0xa3, 0x4d, 0x4e, 0x48,
        0x9d, 0x53, 0xb5, 0x4b, 0x43, 0xc0, 0x61, 0x12, 0x9b, 0xe2, 0xdd, 0x47, 0xbd, 0xce, 0xde,
        0x03, 0x39,
    ],
    status: TrustedKeyStatus::Current,
}];

pub(crate) struct EmbeddedTrustedKeyStore {
    entries: Vec<TrustedKeyEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrustStoreError {
    KeyIdMalformed,
    FingerprintMismatch,
    DuplicateKeyId,
}

impl fmt::Display for TrustStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeyIdMalformed => formatter.write_str("trusted key id is malformed"),
            Self::FingerprintMismatch => {
                formatter.write_str("trusted key fingerprint does not match public key")
            }
            Self::DuplicateKeyId => formatter.write_str("trusted key id is duplicated"),
        }
    }
}

impl std::error::Error for TrustStoreError {}

impl TrustedKeyEntry {
    fn from_embedded(spec: EmbeddedTrustedKeySpec) -> Self {
        Self {
            key_id: spec.key_id.to_string(),
            public_key: spec.public_key,
            status: spec.status,
        }
    }

    #[cfg(test)]
    fn for_test(
        key_id: String,
        public_key: [u8; TRUSTED_PUBLIC_KEY_BYTES],
        status: TrustedKeyStatus,
    ) -> Self {
        Self {
            key_id,
            public_key,
            status,
        }
    }
}

impl EmbeddedTrustedKeyStore {
    pub(crate) fn embedded() -> Result<Self, TrustStoreError> {
        Self::from_entries(
            EMBEDDED_TRUSTED_KEY_TABLE
                .iter()
                .copied()
                .map(TrustedKeyEntry::from_embedded),
        )
    }

    fn from_entries(
        entries: impl IntoIterator<Item = TrustedKeyEntry>,
    ) -> Result<Self, TrustStoreError> {
        let mut validated_entries = Vec::new();
        for entry in entries {
            let expected_fingerprint = parse_key_id_fingerprint(&entry.key_id)
                .map_err(|_| TrustStoreError::KeyIdMalformed)?;
            if public_key_fingerprint(&entry.public_key) != expected_fingerprint {
                return Err(TrustStoreError::FingerprintMismatch);
            }
            if validated_entries.iter().any(|existing: &TrustedKeyEntry| {
                existing.key_id.as_bytes() == entry.key_id.as_bytes()
            }) {
                return Err(TrustStoreError::DuplicateKeyId);
            }
            validated_entries.push(entry);
        }

        Ok(Self {
            entries: validated_entries,
        })
    }

    #[cfg(test)]
    fn from_test_entries(entries: Vec<TrustedKeyEntry>) -> Result<Self, TrustStoreError> {
        Self::from_entries(entries)
    }

    fn lookup(&self, key_id: &str) -> Result<&TrustedKeyEntry, SignatureVerificationError> {
        let expected_fingerprint = parse_key_id_fingerprint(key_id)
            .map_err(|_| SignatureVerificationError::KeyIdMalformed)?;
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.key_id.as_bytes() == key_id.as_bytes())
            .ok_or(SignatureVerificationError::KeyUnknown)?;

        // This is checked again at lookup as a defense-in-depth invariant. A
        // store can only be constructed through the validation above, but a
        // key must never be accepted if its identifier and raw key diverge.
        if public_key_fingerprint(&entry.public_key) != expected_fingerprint {
            return Err(SignatureVerificationError::KeyUnknown);
        }

        match entry.status {
            TrustedKeyStatus::Current | TrustedKeyStatus::Next => Ok(entry),
            TrustedKeyStatus::Retired => Err(SignatureVerificationError::KeyRetired),
            TrustedKeyStatus::Revoked => Err(SignatureVerificationError::KeyRevoked),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignatureVerificationError {
    KeyIdMalformed,
    KeyUnknown,
    KeyRetired,
    KeyRevoked,
    ProofEncoding,
    CanonicalPayload,
    ProofMismatch,
    PackageDigestMismatch,
    PackageSizeMismatch,
}

impl SignatureVerificationError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::KeyIdMalformed => "signature-key-id-malformed",
            Self::KeyUnknown => "signature-key-unknown",
            Self::KeyRetired => "signature-key-retired",
            Self::KeyRevoked => "signature-key-revoked",
            Self::ProofEncoding => "signature-proof-encoding",
            Self::CanonicalPayload => "signature-canonical-payload",
            Self::ProofMismatch => "signature-proof-mismatch",
            Self::PackageDigestMismatch => "package-digest-mismatch",
            Self::PackageSizeMismatch => "package-size-mismatch",
        }
    }
}

impl fmt::Display for SignatureVerificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for SignatureVerificationError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VerifiedSignature;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SignaturePayloadError {
    UnsupportedManifestSchemaVersion {
        actual: u32,
    },
    InvalidManifestDigest,
    ActualDigestMismatch,
    FieldTooLong {
        field: &'static str,
        byte_length: usize,
    },
}

impl fmt::Display for SignaturePayloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedManifestSchemaVersion { actual } => write!(
                formatter,
                "manifest schema version {actual} is unsupported for signature payload"
            ),
            Self::InvalidManifestDigest => formatter
                .write_str("manifest artifact sha256 must be lowercase 64-character hexadecimal"),
            Self::ActualDigestMismatch => {
                formatter.write_str("actual artifact digest does not match manifest sha256")
            }
            Self::FieldTooLong { field, byte_length } => write!(
                formatter,
                "signature payload field {field} is too long for LP32: {byte_length} bytes"
            ),
        }
    }
}

impl std::error::Error for SignaturePayloadError {}

pub(crate) fn encode_signature_payload(
    manifest: &UpdateManifest,
    release: &UpdateRelease,
    actual_digest: [u8; 32],
) -> Result<Vec<u8>, SignaturePayloadError> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(SignaturePayloadError::UnsupportedManifestSchemaVersion {
            actual: manifest.schema_version,
        });
    }

    let manifest_digest = decode_manifest_digest(&release.artifact.sha256)?;
    if manifest_digest != actual_digest {
        return Err(SignaturePayloadError::ActualDigestMismatch);
    }

    let version = release.version.to_string();
    let min_version = release.min_version.to_string();
    let max_version = release
        .max_version_exclusive
        .as_ref()
        .map(ToString::to_string);

    let mut payload = Vec::new();
    payload.extend_from_slice(SIGNATURE_PAYLOAD_DOMAIN_BYTES);
    payload.push(SIGNATURE_PAYLOAD_VERSION);
    payload.extend_from_slice(&MANIFEST_SCHEMA_VERSION.to_be_bytes());
    append_lp32(&mut payload, "productId", &manifest.product_id)?;
    append_lp32(&mut payload, "version", &version)?;
    append_lp32(&mut payload, "channel", &release.channel)?;
    append_lp32(&mut payload, "architecture", &release.architecture)?;
    append_lp32(&mut payload, "minVersion", &min_version)?;
    match max_version.as_deref() {
        None => payload.push(0),
        Some(value) => {
            payload.push(1);
            append_lp32(&mut payload, "maxVersionExclusive", value)?;
        }
    }
    append_lp32(&mut payload, "artifactId", &release.artifact.artifact_id)?;
    append_lp32(&mut payload, "format", &release.artifact.format)?;
    payload.extend_from_slice(&release.artifact.size_bytes.to_be_bytes());
    payload.extend_from_slice(&manifest_digest);

    Ok(payload)
}

pub(crate) fn signed_release_identity_sha256(
    manifest: &UpdateManifest,
    release: &UpdateRelease,
) -> Result<String, SignaturePayloadError> {
    let manifest_digest = decode_manifest_digest(&release.artifact.sha256)?;
    let canonical_payload = encode_signature_payload(manifest, release, manifest_digest)?;

    let mut identity = Vec::with_capacity(
        SIGNED_IDENTITY_DOMAIN_BYTES.len()
            + canonical_payload.len()
            + release.signature.key_id.len()
            + release.signature.proof.len()
            + 24,
    );
    identity.extend_from_slice(SIGNED_IDENTITY_DOMAIN_BYTES);
    append_identity_part(&mut identity, &canonical_payload);
    append_identity_part(&mut identity, release.signature.key_id.as_bytes());
    append_identity_part(&mut identity, release.signature.proof.as_bytes());

    Ok(hex_digest(digest(&SHA256, &identity).as_ref()))
}

pub(crate) fn verify_selected_package(
    manifest_root: &UpdateManifest,
    selected_release: &UpdateRelease,
    actual_size_bytes: u64,
    actual_sha256: [u8; 32],
    trust_store: &EmbeddedTrustedKeyStore,
) -> Result<VerifiedSignature, SignatureVerificationError> {
    let trusted_key = trust_store.lookup(&selected_release.signature.key_id)?;

    if actual_size_bytes != selected_release.artifact.size_bytes {
        return Err(SignatureVerificationError::PackageSizeMismatch);
    }

    let payload = encode_signature_payload(manifest_root, selected_release, actual_sha256)
        .map_err(|error| match error {
            SignaturePayloadError::ActualDigestMismatch => {
                SignatureVerificationError::PackageDigestMismatch
            }
            SignaturePayloadError::UnsupportedManifestSchemaVersion { .. }
            | SignaturePayloadError::InvalidManifestDigest
            | SignaturePayloadError::FieldTooLong { .. } => {
                SignatureVerificationError::CanonicalPayload
            }
        })?;

    let signature = decode_signature_proof(&selected_release.signature.proof)?;
    UnparsedPublicKey::new(&ED25519, trusted_key.public_key.as_ref())
        .verify(&payload, &signature)
        .map_err(|_| SignatureVerificationError::ProofMismatch)?;

    Ok(VerifiedSignature)
}

fn decode_signature_proof(
    proof: &str,
) -> Result<[u8; ED25519_SIGNATURE_BYTES], SignatureVerificationError> {
    if !proof.as_bytes().iter().copied().all(is_base64url_byte) {
        return Err(SignatureVerificationError::ProofEncoding);
    }

    let decoded = URL_SAFE_NO_PAD
        .decode(proof)
        .map_err(|_| SignatureVerificationError::ProofEncoding)?;
    if decoded.len() != ED25519_SIGNATURE_BYTES {
        return Err(SignatureVerificationError::ProofEncoding);
    }

    let mut signature = [0u8; ED25519_SIGNATURE_BYTES];
    signature.copy_from_slice(&decoded);
    if URL_SAFE_NO_PAD.encode(signature) != proof {
        return Err(SignatureVerificationError::ProofEncoding);
    }

    Ok(signature)
}

fn append_identity_part(identity: &mut Vec<u8>, value: &[u8]) {
    identity.extend_from_slice(&(value.len() as u64).to_be_bytes());
    identity.extend_from_slice(value);
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_base64url_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

fn parse_key_id_fingerprint(value: &str) -> Result<[u8; TRUSTED_PUBLIC_KEY_BYTES], ()> {
    if !value.is_ascii() {
        return Err(());
    }
    let Some(suffix) = value.strip_prefix(TRUSTED_KEY_ID_PREFIX) else {
        return Err(());
    };
    if suffix.len() != TRUSTED_PUBLIC_KEY_BYTES * 2
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(());
    }

    let mut fingerprint = [0u8; TRUSTED_PUBLIC_KEY_BYTES];
    for (index, pair) in suffix.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_hex_nibble(pair[0]).ok_or(())?;
        let low = decode_hex_nibble(pair[1]).ok_or(())?;
        fingerprint[index] = (high << 4) | low;
    }
    Ok(fingerprint)
}

fn public_key_fingerprint(
    public_key: &[u8; TRUSTED_PUBLIC_KEY_BYTES],
) -> [u8; TRUSTED_PUBLIC_KEY_BYTES] {
    let digest = digest(&SHA256, public_key);
    let mut fingerprint = [0u8; TRUSTED_PUBLIC_KEY_BYTES];
    fingerprint.copy_from_slice(digest.as_ref());
    fingerprint
}

fn append_lp32(
    payload: &mut Vec<u8>,
    field: &'static str,
    value: &str,
) -> Result<(), SignaturePayloadError> {
    let bytes = value.as_bytes();
    let length = u32::try_from(bytes.len()).map_err(|_| SignaturePayloadError::FieldTooLong {
        field,
        byte_length: bytes.len(),
    })?;
    payload.extend_from_slice(&length.to_be_bytes());
    payload.extend_from_slice(bytes);
    Ok(())
}

fn decode_manifest_digest(value: &str) -> Result<[u8; 32], SignaturePayloadError> {
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        return Err(SignaturePayloadError::InvalidManifestDigest);
    }

    let mut digest = [0u8; 32];
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        let high =
            decode_hex_nibble(pair[0]).ok_or(SignaturePayloadError::InvalidManifestDigest)?;
        let low = decode_hex_nibble(pair[1]).ok_or(SignaturePayloadError::InvalidManifestDigest)?;
        digest[index] = (high << 4) | low;
    }
    Ok(digest)
}

fn decode_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update_manifest::{parse_manifest, MacOsVersion, SemVer};
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use serde::Deserialize;

    const ACTUAL_DIGEST: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];
    const VALID_MANIFEST: &str = r#"
        {
          "productId": "com.cornellmethod.notebook",
          "schemaVersion": 1,
          "releases": [{
            "channel": "stable",
            "version": "1.2.3+build.7",
            "architecture": "aarch64-apple-darwin",
            "minVersion": "14.0",
            "maxVersionExclusive": "15.0",
            "artifact": {
              "artifactId": "com.cornellmethod.notebook-1.2.3",
              "format": "app-archive",
              "url": "https://updates.example.test/cornell-method/app",
              "sizeBytes": 12345,
              "sha256": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
            },
            "signature": { "keyId": "current-key", "proof": "opaque-proof" }
          }]
        }
    "#;
    const EXPECTED_PAYLOAD_HEX: &str = "636f6d2e636f726e656c6c6d6574686f642e6e6f7465626f6f6b2f6465736b746f702d7570646174652d7369676e61747572652f656432353531390001000000010000001a636f6d2e636f726e656c6c6d6574686f642e6e6f7465626f6f6b0000000d312e322e332b6275696c642e3700000006737461626c6500000014616172636836342d6170706c652d64617277696e0000000231340100000002313500000020636f6d2e636f726e656c6c6d6574686f642e6e6f7465626f6f6b2d312e322e330000000b6170702d617263686976650000000000003039000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    const UPDATE_SIGNATURE_FIXTURE_JSON: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test/desktop/fixtures/update-signature/valid.json"
    ));

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct InteroperabilityFixture {
        #[serde(rename = "fixtureVersion")]
        fixture_version: u32,
        #[serde(rename = "manifestSchemaVersion")]
        manifest_schema_version: u32,
        #[serde(rename = "payloadVersion")]
        payload_version: u8,
        release: InteroperabilityRelease,
        #[serde(rename = "keyId")]
        key_id: String,
        #[serde(rename = "publicKeyHex")]
        public_key_hex: String,
        #[serde(rename = "canonicalPayloadHex")]
        canonical_payload_hex: String,
        proof: String,
        expected: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct InteroperabilityRelease {
        #[serde(rename = "productId")]
        product_id: String,
        version: String,
        channel: String,
        architecture: String,
        #[serde(rename = "minVersion")]
        min_version: String,
        #[serde(rename = "maxVersionExclusive")]
        max_version_exclusive: Option<String>,
        #[serde(rename = "artifactId")]
        artifact_id: String,
        format: String,
        #[serde(rename = "sizeBytes")]
        size_bytes: u64,
        sha256: String,
    }

    fn parsed_manifest() -> UpdateManifest {
        parse_manifest(VALID_MANIFEST).unwrap()
    }

    fn interoperability_fixture() -> InteroperabilityFixture {
        serde_json::from_str(UPDATE_SIGNATURE_FIXTURE_JSON).unwrap()
    }

    fn manifest_from_interoperability_fixture(fixture: &InteroperabilityFixture) -> UpdateManifest {
        let release = &fixture.release;
        let manifest_json = serde_json::json!({
            "productId": release.product_id,
            "schemaVersion": fixture.manifest_schema_version,
            "releases": [{
                "channel": release.channel,
                "version": release.version,
                "architecture": release.architecture,
                "minVersion": release.min_version,
                "maxVersionExclusive": release.max_version_exclusive,
                "artifact": {
                    "artifactId": release.artifact_id,
                    "format": release.format,
                    "url": "https://updates.example.test/cornell-method/app",
                    "sizeBytes": release.size_bytes,
                    "sha256": release.sha256,
                },
                "signature": {
                    "keyId": fixture.key_id,
                    "proof": fixture.proof,
                },
            }]
        });
        parse_manifest(&manifest_json.to_string()).unwrap()
    }

    fn decode_fixture_hex(value: &str) -> Vec<u8> {
        assert_eq!(value.len() % 2, 0);
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let high = decode_hex_nibble(pair[0]).expect("fixture hex must be lowercase");
                let low = decode_hex_nibble(pair[1]).expect("fixture hex must be lowercase");
                (high << 4) | low
            })
            .collect()
    }

    fn mutate_key_id(key_id: &str) -> String {
        let mut bytes = key_id.as_bytes().to_vec();
        let last = bytes.last_mut().expect("fixture key id must not be empty");
        *last = if *last == b'0' { b'1' } else { b'0' };
        String::from_utf8(bytes).unwrap()
    }

    fn encode_fixture(manifest: &UpdateManifest) -> Result<Vec<u8>, SignaturePayloadError> {
        encode_signature_payload(manifest, &manifest.releases[0], ACTUAL_DIGEST)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn assert_changed_or_rejected(
        original_payload: &[u8],
        result: Result<Vec<u8>, SignaturePayloadError>,
    ) {
        assert!(
            result.map_or(true, |payload| payload != original_payload),
            "mutation must change the payload or be rejected"
        );
    }

    struct EphemeralTestKey {
        key_pair: Ed25519KeyPair,
        public_key: [u8; TRUSTED_PUBLIC_KEY_BYTES],
    }

    impl EphemeralTestKey {
        fn generate() -> Self {
            let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
            let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
            let public_key = key_pair
                .public_key()
                .as_ref()
                .try_into()
                .expect("Ed25519 public key must be 32 bytes");
            Self {
                key_pair,
                public_key,
            }
        }

        fn key_id(&self) -> String {
            format!(
                "{TRUSTED_KEY_ID_PREFIX}{}",
                hex(&public_key_fingerprint(&self.public_key))
            )
        }

        fn entry(&self, status: TrustedKeyStatus) -> TrustedKeyEntry {
            TrustedKeyEntry::for_test(self.key_id(), self.public_key, status)
        }

        fn sign(&self, payload: &[u8]) -> String {
            URL_SAFE_NO_PAD.encode(self.key_pair.sign(payload).as_ref())
        }
    }

    fn signed_manifest(key: &EphemeralTestKey) -> UpdateManifest {
        let mut manifest = parsed_manifest();
        manifest.releases[0].signature.key_id = key.key_id();
        let payload =
            encode_signature_payload(&manifest, &manifest.releases[0], ACTUAL_DIGEST).unwrap();
        manifest.releases[0].signature.proof = key.sign(&payload);
        manifest
    }

    fn test_store(key: &EphemeralTestKey, status: TrustedKeyStatus) -> EmbeddedTrustedKeyStore {
        EmbeddedTrustedKeyStore::from_test_entries(vec![key.entry(status)]).unwrap()
    }

    fn verify_fixture(
        manifest: &UpdateManifest,
        store: &EmbeddedTrustedKeyStore,
        actual_size_bytes: u64,
        actual_sha256: [u8; 32],
    ) -> Result<VerifiedSignature, SignatureVerificationError> {
        verify_selected_package(
            manifest,
            &manifest.releases[0],
            actual_size_bytes,
            actual_sha256,
            store,
        )
    }

    fn replace_first_proof_byte(proof: &str) -> String {
        let replacement = if proof.as_bytes()[0] == b'A' {
            b'B'
        } else {
            b'A'
        };
        let mut bytes = proof.as_bytes().to_vec();
        bytes[0] = replacement;
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn encodes_the_v1_known_answer_with_exact_binary_order() {
        assert_eq!(SIGNATURE_PAYLOAD_DOMAIN_BYTES.last(), Some(&0));
        let manifest = parsed_manifest();
        let payload = encode_fixture(&manifest).unwrap();

        assert_eq!(hex(&payload), EXPECTED_PAYLOAD_HEX);
        assert_eq!(
            payload[SIGNATURE_PAYLOAD_DOMAIN_BYTES.len()],
            SIGNATURE_PAYLOAD_VERSION
        );
        assert_eq!(manifest.releases[0].version.to_string(), "1.2.3+build.7");
        assert_eq!(manifest.releases[0].min_version.to_string(), "14");
        assert_eq!(
            manifest.releases[0]
                .max_version_exclusive
                .as_ref()
                .unwrap()
                .to_string(),
            "15"
        );
        assert_eq!(
            &payload[payload.len() - ACTUAL_DIGEST.len()..],
            &ACTUAL_DIGEST
        );
    }

    #[test]
    fn signed_identity_binds_range_and_proof_without_changing_wire_payload() {
        let manifest = parsed_manifest();
        let release = &manifest.releases[0];
        let original = signed_release_identity_sha256(&manifest, release).unwrap();
        assert_eq!(original.len(), 64);

        let mut min_changed = release.clone();
        min_changed.min_version = MacOsVersion::parse("15", "test min version").unwrap();
        assert_ne!(
            original,
            signed_release_identity_sha256(&manifest, &min_changed).unwrap()
        );

        let mut max_changed = release.clone();
        max_changed.max_version_exclusive =
            Some(MacOsVersion::parse("16", "test max version").unwrap());
        assert_ne!(
            original,
            signed_release_identity_sha256(&manifest, &max_changed).unwrap()
        );

        let mut proof_changed = release.clone();
        proof_changed.signature.proof = "different-proof".to_string();
        assert_ne!(
            original,
            signed_release_identity_sha256(&manifest, &proof_changed).unwrap()
        );
    }

    #[test]
    fn ignores_json_representation_and_unsigned_release_metadata() {
        let reordered = r#"{
          "releases":[{
            "signature":{"proof":"different-proof","keyId":"different-key"},
            "artifact":{
              "sha256":"000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
              "sizeBytes":12345,
              "url":"https://cdn.example.test/another-name",
              "format":"app-archive",
              "artifactId":"com.cornellmethod.notebook-1.2.3"
            },
            "maxVersionExclusive":"15.0",
            "minVersion":"14.0",
            "architecture":"aarch64-apple-darwin",
            "version":"1.2.3+build.7",
            "channel":"stable"
          }],
          "schemaVersion":1,
          "productId":"com.cornellmethod.notebook"
        }"#;

        let original = parsed_manifest();
        let alternate = parse_manifest(reordered).unwrap();
        assert_eq!(
            encode_fixture(&original).unwrap(),
            encode_fixture(&alternate).unwrap()
        );
    }

    #[test]
    fn binds_all_signed_fields_and_schema_version() {
        let original = parsed_manifest();
        let original_payload = encode_fixture(&original).unwrap();

        let mut version = original.clone();
        version.releases[0].version = SemVer::parse("1.2.4+build.7").unwrap();
        assert_changed_or_rejected(&original_payload, encode_fixture(&version));

        let mut channel = original.clone();
        channel.releases[0].channel = "beta".to_string();
        assert_changed_or_rejected(&original_payload, encode_fixture(&channel));

        let mut architecture = original.clone();
        architecture.releases[0].architecture = "x86_64-apple-darwin".to_string();
        assert_changed_or_rejected(&original_payload, encode_fixture(&architecture));

        let mut min_version = original.clone();
        min_version.releases[0].min_version =
            MacOsVersion::parse("14.1", "test minVersion").unwrap();
        assert_changed_or_rejected(&original_payload, encode_fixture(&min_version));

        let mut max_version = original.clone();
        max_version.releases[0].max_version_exclusive =
            Some(MacOsVersion::parse("16", "test maxVersionExclusive").unwrap());
        assert_changed_or_rejected(&original_payload, encode_fixture(&max_version));

        let mut artifact_id = original.clone();
        artifact_id.releases[0].artifact.artifact_id = "different-artifact".to_string();
        assert_changed_or_rejected(&original_payload, encode_fixture(&artifact_id));

        let mut format = original.clone();
        format.releases[0].artifact.format = "different-format".to_string();
        assert_changed_or_rejected(&original_payload, encode_fixture(&format));

        let mut size = original.clone();
        size.releases[0].artifact.size_bytes = 12346;
        assert_changed_or_rejected(&original_payload, encode_fixture(&size));

        let mut product_id = original.clone();
        product_id.product_id = "com.example.other".to_string();
        assert_changed_or_rejected(&original_payload, encode_fixture(&product_id));

        let mut digest = original.clone();
        digest.releases[0].artifact.sha256 =
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();
        assert_changed_or_rejected(&original_payload, encode_fixture(&digest));
        assert_changed_or_rejected(
            &original_payload,
            encode_signature_payload(&digest, &digest.releases[0], [0xff; 32]),
        );

        let mut unsupported_schema = original;
        unsupported_schema.schema_version = 2;
        assert_eq!(
            encode_fixture(&unsupported_schema),
            Err(SignaturePayloadError::UnsupportedManifestSchemaVersion { actual: 2 })
        );
    }

    #[test]
    fn distinguishes_absent_and_present_max_version() {
        let mut absent = parsed_manifest();
        absent.releases[0].max_version_exclusive = None;
        let present = parsed_manifest();

        let absent_payload = encode_fixture(&absent).unwrap();
        let present_payload = encode_fixture(&present).unwrap();
        assert_ne!(absent_payload, present_payload);
    }

    #[test]
    fn rejects_malformed_manifest_digest_and_actual_digest_mismatch() {
        for invalid_digest in [
            "short",
            "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg",
        ] {
            let mut manifest = parsed_manifest();
            manifest.releases[0].artifact.sha256 = invalid_digest.to_string();
            assert_eq!(
                encode_fixture(&manifest),
                Err(SignaturePayloadError::InvalidManifestDigest)
            );
        }

        let manifest = parsed_manifest();
        let result = encode_signature_payload(&manifest, &manifest.releases[0], [0xff; 32]);
        assert_eq!(result, Err(SignaturePayloadError::ActualDigestMismatch));
        assert!(result.is_err());
    }

    #[test]
    fn embedded_store_contains_the_approved_current_production_key() {
        let store = EmbeddedTrustedKeyStore::embedded().unwrap();
        assert_eq!(store.entries.len(), 1);

        let entry = store
            .lookup(
                "cmn-ed25519-v1-381374c2723e7e3624ed21bd2836992ae9266ef78dfeb9ac21b33a08e8632f54",
            )
            .unwrap();
        assert_eq!(
            entry.key_id,
            "cmn-ed25519-v1-381374c2723e7e3624ed21bd2836992ae9266ef78dfeb9ac21b33a08e8632f54"
        );
        assert_eq!(
            entry.public_key,
            [
                0xf1, 0xca, 0xe2, 0x84, 0x7e, 0x46, 0x9f, 0x73, 0x66, 0x23, 0x0a, 0xa3, 0x4d, 0x4e,
                0x48, 0x9d, 0x53, 0xb5, 0x4b, 0x43, 0xc0, 0x61, 0x12, 0x9b, 0xe2, 0xdd, 0x47, 0xbd,
                0xce, 0xde, 0x03, 0x39,
            ]
        );
        assert_eq!(entry.status, TrustedKeyStatus::Current);
    }

    #[test]
    fn verifies_signatures_from_current_and_next_keys() {
        for status in [TrustedKeyStatus::Current, TrustedKeyStatus::Next] {
            let key = EphemeralTestKey::generate();
            let manifest = signed_manifest(&key);
            let store = test_store(&key, status);

            assert_eq!(
                verify_fixture(&manifest, &store, 12345, ACTUAL_DIGEST),
                Ok(VerifiedSignature)
            );
        }
    }

    #[test]
    fn rejects_retired_and_revoked_keys_with_distinct_codes() {
        for (status, code) in [
            (TrustedKeyStatus::Retired, "signature-key-retired"),
            (TrustedKeyStatus::Revoked, "signature-key-revoked"),
        ] {
            let key = EphemeralTestKey::generate();
            let manifest = signed_manifest(&key);
            let store = test_store(&key, status);
            let error = verify_fixture(&manifest, &store, 12345, ACTUAL_DIGEST).unwrap_err();

            assert_eq!(error.code(), code);
        }
    }

    #[test]
    fn rejects_malformed_and_unknown_key_ids_before_package_or_proof_work() {
        let key = EphemeralTestKey::generate();
        let manifest = signed_manifest(&key);
        let store = test_store(&key, TrustedKeyStatus::Current);
        let valid_key_id = key.key_id();
        let malformed_ids = [
            "current-key".to_string(),
            format!("{TRUSTED_KEY_ID_PREFIX}{}", "a".repeat(63)),
            format!("{TRUSTED_KEY_ID_PREFIX}{}", "A".repeat(64)),
            format!(" {valid_key_id}"),
            format!("{valid_key_id} "),
            format!("{TRUSTED_KEY_ID_PREFIX}{}", "a".repeat(63) + "%"),
        ];

        for key_id in malformed_ids {
            let mut candidate = manifest.clone();
            candidate.releases[0].signature.key_id = key_id;
            let error = verify_fixture(&candidate, &store, 0, [0xff; 32]).unwrap_err();
            assert_eq!(error.code(), "signature-key-id-malformed");
        }

        let unknown_key = EphemeralTestKey::generate();
        let mut candidate = manifest;
        candidate.releases[0].signature.key_id = unknown_key.key_id();
        let error = verify_fixture(&candidate, &store, 0, [0xff; 32]).unwrap_err();
        assert_eq!(error.code(), "signature-key-unknown");
    }

    #[test]
    fn rejects_duplicate_ids_and_fingerprint_mismatches_when_building_store() {
        let first = EphemeralTestKey::generate();
        let second = EphemeralTestKey::generate();

        let duplicate = EmbeddedTrustedKeyStore::from_test_entries(vec![
            first.entry(TrustedKeyStatus::Current),
            first.entry(TrustedKeyStatus::Next),
        ])
        .err()
        .unwrap();
        assert_eq!(duplicate, TrustStoreError::DuplicateKeyId);

        let mismatched =
            EmbeddedTrustedKeyStore::from_test_entries(vec![TrustedKeyEntry::for_test(
                first.key_id(),
                second.public_key,
                TrustedKeyStatus::Current,
            )])
            .err()
            .unwrap();
        assert_eq!(mismatched, TrustStoreError::FingerprintMismatch);
    }

    #[test]
    fn verifies_strict_base64url_and_rejects_noncanonical_proofs() {
        let key = EphemeralTestKey::generate();
        let manifest = signed_manifest(&key);
        let store = test_store(&key, TrustedKeyStatus::Current);
        let invalid_proofs = [
            "!!!!".to_string(),
            "AA=".to_string(),
            "AA BB".to_string(),
            "AA%20".to_string(),
            "ed25519:proof".to_string(),
            r#"{"proof":"ignored"}"#.to_string(),
            r#"["proof-one","proof-two"]"#.to_string(),
            URL_SAFE_NO_PAD.encode([0u8; 63]),
        ];

        for proof in invalid_proofs {
            let mut candidate = manifest.clone();
            candidate.releases[0].signature.proof = proof;
            let error = verify_fixture(&candidate, &store, 12345, ACTUAL_DIGEST).unwrap_err();
            assert_eq!(error.code(), "signature-proof-encoding");
        }

        let mut noncanonical = manifest.clone();
        let mut proof = noncanonical.releases[0].signature.proof.as_bytes().to_vec();
        *proof.last_mut().unwrap() = b'B';
        noncanonical.releases[0].signature.proof = String::from_utf8(proof).unwrap();
        let error = verify_fixture(&noncanonical, &store, 12345, ACTUAL_DIGEST).unwrap_err();
        assert_eq!(error.code(), "signature-proof-encoding");
    }

    #[test]
    fn rejects_size_and_digest_mismatches_before_signature_verification() {
        let key = EphemeralTestKey::generate();
        let manifest = signed_manifest(&key);
        let store = test_store(&key, TrustedKeyStatus::Current);

        let size_error = verify_fixture(&manifest, &store, 12346, [0xff; 32]).unwrap_err();
        assert_eq!(size_error.code(), "package-size-mismatch");

        let digest_error = verify_fixture(&manifest, &store, 12345, [0xff; 32]).unwrap_err();
        assert_eq!(digest_error.code(), "package-digest-mismatch");
    }

    #[test]
    fn payload_and_signature_mutations_fail_with_proof_mismatch() {
        let key = EphemeralTestKey::generate();
        let manifest = signed_manifest(&key);
        let store = test_store(&key, TrustedKeyStatus::Current);

        let mut payload_mutation = manifest.clone();
        payload_mutation.releases[0].version = SemVer::parse("1.2.4+build.7").unwrap();
        let payload_error =
            verify_fixture(&payload_mutation, &store, 12345, ACTUAL_DIGEST).unwrap_err();
        assert_eq!(payload_error.code(), "signature-proof-mismatch");

        let mut proof_mutation = manifest;
        proof_mutation.releases[0].signature.proof =
            replace_first_proof_byte(&proof_mutation.releases[0].signature.proof);
        let proof_error =
            verify_fixture(&proof_mutation, &store, 12345, ACTUAL_DIGEST).unwrap_err();
        assert_eq!(proof_error.code(), "signature-proof-mismatch");
    }

    #[test]
    fn verifies_checked_in_interoperability_fixture_and_fails_closed_for_mutations() {
        let fixture = interoperability_fixture();
        assert_eq!(fixture.fixture_version, 1);
        assert_eq!(fixture.manifest_schema_version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(fixture.payload_version, SIGNATURE_PAYLOAD_VERSION);
        assert_eq!(fixture.expected, "valid");
        assert_eq!(fixture.release.product_id, "com.cornellmethod.notebook");
        assert_eq!(fixture.release.size_bytes, 12345);

        let actual_digest: [u8; 32] = decode_fixture_hex(&fixture.release.sha256)
            .try_into()
            .expect("fixture digest must be 32 bytes");
        assert_eq!(actual_digest, ACTUAL_DIGEST);

        let public_key: [u8; TRUSTED_PUBLIC_KEY_BYTES] =
            decode_fixture_hex(&fixture.public_key_hex)
                .try_into()
                .expect("fixture public key must be 32 bytes");
        assert_eq!(
            fixture.key_id,
            format!(
                "{TRUSTED_KEY_ID_PREFIX}{}",
                hex(&public_key_fingerprint(&public_key))
            )
        );

        let manifest = manifest_from_interoperability_fixture(&fixture);
        let payload =
            encode_signature_payload(&manifest, &manifest.releases[0], actual_digest).unwrap();
        assert_eq!(hex(&payload), fixture.canonical_payload_hex);
        assert_eq!(decode_signature_proof(&fixture.proof).unwrap().len(), 64);

        let store = EmbeddedTrustedKeyStore::from_test_entries(vec![TrustedKeyEntry::for_test(
            fixture.key_id.clone(),
            public_key,
            TrustedKeyStatus::Current,
        )])
        .unwrap();
        assert_eq!(
            verify_fixture(&manifest, &store, fixture.release.size_bytes, actual_digest,),
            Ok(VerifiedSignature)
        );

        let mut version_mutation = manifest.clone();
        version_mutation.releases[0].version = SemVer::parse("1.2.4+build.7").unwrap();
        assert_eq!(
            verify_fixture(
                &version_mutation,
                &store,
                fixture.release.size_bytes,
                actual_digest,
            )
            .unwrap_err()
            .code(),
            "signature-proof-mismatch"
        );

        assert_eq!(
            verify_fixture(
                &manifest,
                &store,
                fixture.release.size_bytes + 1,
                actual_digest,
            )
            .unwrap_err()
            .code(),
            "package-size-mismatch"
        );

        assert_eq!(
            verify_fixture(&manifest, &store, fixture.release.size_bytes, [0xff; 32])
                .unwrap_err()
                .code(),
            "package-digest-mismatch"
        );

        let mut payload_mutation = manifest.clone();
        payload_mutation.releases[0].artifact.artifact_id = "different-artifact".to_string();
        assert_eq!(
            verify_fixture(
                &payload_mutation,
                &store,
                fixture.release.size_bytes,
                actual_digest,
            )
            .unwrap_err()
            .code(),
            "signature-proof-mismatch"
        );

        let mut proof_mutation = manifest.clone();
        proof_mutation.releases[0].signature.proof =
            replace_first_proof_byte(&proof_mutation.releases[0].signature.proof);
        assert_eq!(
            verify_fixture(
                &proof_mutation,
                &store,
                fixture.release.size_bytes,
                actual_digest,
            )
            .unwrap_err()
            .code(),
            "signature-proof-mismatch"
        );

        let mut key_id_mutation = manifest;
        key_id_mutation.releases[0].signature.key_id = mutate_key_id(&fixture.key_id);
        assert_eq!(
            verify_fixture(
                &key_id_mutation,
                &store,
                fixture.release.size_bytes,
                actual_digest,
            )
            .unwrap_err()
            .code(),
            "signature-key-unknown"
        );
    }

    #[test]
    fn maps_payload_construction_failures_to_canonical_payload() {
        let key = EphemeralTestKey::generate();
        let mut manifest = signed_manifest(&key);
        manifest.schema_version = 2;
        let store = test_store(&key, TrustedKeyStatus::Current);
        let error = verify_fixture(&manifest, &store, 12345, ACTUAL_DIGEST).unwrap_err();

        assert_eq!(error.code(), "signature-canonical-payload");
    }

    #[test]
    fn url_is_not_signed_but_key_id_lookup_remains_exact() {
        let key = EphemeralTestKey::generate();
        let manifest = signed_manifest(&key);
        let store = test_store(&key, TrustedKeyStatus::Current);
        let alternate_json = VALID_MANIFEST.replace(
            "https://updates.example.test/cornell-method/app",
            "https://cdn.example.test/another-name",
        );
        let mut url_changed = parse_manifest(&alternate_json).unwrap();
        url_changed.releases[0].signature = manifest.releases[0].signature.clone();

        assert_eq!(
            verify_fixture(&url_changed, &store, 12345, ACTUAL_DIGEST),
            Ok(VerifiedSignature)
        );

        let unknown_key = EphemeralTestKey::generate();
        let mut key_id_changed = manifest;
        key_id_changed.releases[0].signature.key_id = unknown_key.key_id();
        let error = verify_fixture(&key_id_changed, &store, 12345, ACTUAL_DIGEST).unwrap_err();
        assert_eq!(error.code(), "signature-key-unknown");
    }

    #[test]
    fn verification_errors_do_not_format_sensitive_material() {
        let error = SignatureVerificationError::ProofEncoding;
        let display = error.to_string();
        let debug = format!("{error:?}");

        assert_eq!(display, "signature-proof-encoding");
        assert_eq!(debug, "ProofEncoding");
    }
}
