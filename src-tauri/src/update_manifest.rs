use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Deserializer};
use tauri::Url;

pub(crate) const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub(crate) const MANIFEST_PRODUCT_ID: &str = "com.cornellmethod.notebook";
pub(crate) const TARGET_CHANNEL: &str = "stable";
pub(crate) const TARGET_ARCHITECTURE: &str = "aarch64-apple-darwin";
pub(crate) const TARGET_ARTIFACT_FORMAT: &str = "app-archive";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ManifestValidationError {
    message: String,
}

impl ManifestValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ManifestValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ManifestValidationError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdateManifest {
    pub(crate) product_id: String,
    pub(crate) schema_version: u32,
    pub(crate) releases: Vec<UpdateRelease>,
}

impl UpdateManifest {
    #[cfg(test)]
    pub(crate) fn is_no_update(&self) -> bool {
        self.releases.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdateRelease {
    pub(crate) channel: String,
    pub(crate) version: SemVer,
    pub(crate) architecture: String,
    pub(crate) min_version: MacOsVersion,
    pub(crate) max_version_exclusive: Option<MacOsVersion>,
    pub(crate) artifact: UpdateArtifact,
    pub(crate) signature: UpdateSignature,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdateArtifact {
    pub(crate) artifact_id: String,
    pub(crate) format: String,
    pub(crate) url: HttpsUrl,
    pub(crate) size_bytes: u64,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdateSignature {
    pub(crate) key_id: String,
    pub(crate) proof: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HttpsUrl(String);

impl HttpsUrl {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    fn parse(value: String) -> Result<Self, ManifestValidationError> {
        validate_non_empty_text(&value, "artifact URL")?;

        let parsed = Url::parse(&value).map_err(|_| {
            ManifestValidationError::new("artifact URL must be a valid direct HTTPS URL")
        })?;
        if !is_safe_public_update_url(&parsed) {
            return Err(ManifestValidationError::new(
                "artifact URL must be a direct HTTPS URL without credentials, query parameters, or fragments",
            ));
        }

        Ok(Self(value))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Decimal(String);

impl Decimal {
    fn parse(
        value: &str,
        field: &str,
        allow_leading_zero: bool,
    ) -> Result<Self, ManifestValidationError> {
        if value.is_empty()
            || !value.bytes().all(|byte| byte.is_ascii_digit())
            || (!allow_leading_zero && value.len() > 1 && value.starts_with('0'))
        {
            return Err(ManifestValidationError::new(format!(
                "{field} must contain only valid numeric components"
            )));
        }

        let first_non_zero = value
            .bytes()
            .position(|byte| byte != b'0')
            .unwrap_or(value.len().saturating_sub(1));
        Ok(Self(value[first_non_zero..].to_string()))
    }

    fn zero() -> Self {
        Self("0".to_string())
    }

    fn is_zero(&self) -> bool {
        self.0 == "0"
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.as_bytes().cmp(other.0.as_bytes()))
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SemVer {
    major: Decimal,
    minor: Decimal,
    patch: Decimal,
    pre_release: Vec<PreReleaseIdentifier>,
    build_metadata: Vec<String>,
}

impl SemVer {
    pub(crate) fn parse(value: &str) -> Result<Self, ManifestValidationError> {
        validate_non_empty_text(value, "version")?;

        let (without_build, build_metadata) = value
            .split_once('+')
            .map_or((value, None), |(version, build)| (version, Some(build)));
        let (core, pre_release) = without_build
            .split_once('-')
            .map_or((without_build, None), |(core, pre)| (core, Some(pre)));
        let mut core_components = core.split('.');
        let major = Decimal::parse(
            core_components
                .next()
                .ok_or_else(|| ManifestValidationError::new("version is not valid SemVer"))?,
            "version",
            false,
        )?;
        let minor = Decimal::parse(
            core_components
                .next()
                .ok_or_else(|| ManifestValidationError::new("version is not valid SemVer"))?,
            "version",
            false,
        )?;
        let patch = Decimal::parse(
            core_components
                .next()
                .ok_or_else(|| ManifestValidationError::new("version is not valid SemVer"))?,
            "version",
            false,
        )?;
        if core_components.next().is_some() {
            return Err(ManifestValidationError::new("version is not valid SemVer"));
        }

        let pre_release = pre_release
            .map(parse_pre_release)
            .transpose()?
            .unwrap_or_default();
        let build_metadata = build_metadata
            .map(parse_build_metadata)
            .transpose()?
            .unwrap_or_default();

        Ok(Self {
            major,
            minor,
            patch,
            pre_release,
            build_metadata,
        })
    }

    pub(crate) fn is_prerelease(&self) -> bool {
        !self.pre_release.is_empty()
    }

    pub(crate) fn precedence_cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| compare_pre_release(&self.pre_release, &other.pre_release))
    }
}

impl PartialEq for SemVer {
    fn eq(&self, other: &Self) -> bool {
        self.precedence_cmp(other) == Ordering::Equal
    }
}

impl Eq for SemVer {}

impl Hash for SemVer {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.major.hash(state);
        self.minor.hash(state);
        self.patch.hash(state);
        self.pre_release.hash(state);
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        self.precedence_cmp(other)
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for SemVer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre_release.is_empty() {
            formatter.write_str("-")?;
            for (index, identifier) in self.pre_release.iter().enumerate() {
                if index > 0 {
                    formatter.write_str(".")?;
                }
                identifier.fmt(formatter)?;
            }
        }
        if !self.build_metadata.is_empty() {
            formatter.write_str("+")?;
            formatter.write_str(&self.build_metadata.join("."))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum PreReleaseIdentifier {
    Numeric(Decimal),
    Text(String),
}

impl Ord for PreReleaseIdentifier {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Numeric(left), Self::Numeric(right)) => left.cmp(right),
            (Self::Numeric(_), Self::Text(_)) => Ordering::Less,
            (Self::Text(_), Self::Numeric(_)) => Ordering::Greater,
            (Self::Text(left), Self::Text(right)) => left.as_bytes().cmp(right.as_bytes()),
        }
    }
}

impl PartialOrd for PreReleaseIdentifier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for PreReleaseIdentifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Numeric(value) => value.fmt(formatter),
            Self::Text(value) => formatter.write_str(value),
        }
    }
}

fn compare_pre_release(left: &[PreReleaseIdentifier], right: &[PreReleaseIdentifier]) -> Ordering {
    match (left.is_empty(), right.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => left
            .iter()
            .zip(right)
            .map(|(left, right)| left.cmp(right))
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or_else(|| left.len().cmp(&right.len())),
    }
}

fn parse_pre_release(value: &str) -> Result<Vec<PreReleaseIdentifier>, ManifestValidationError> {
    if value.is_empty() {
        return Err(ManifestValidationError::new(
            "version prerelease must not be empty",
        ));
    }

    value
        .split('.')
        .map(|identifier| {
            if identifier.is_empty()
                || !identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err(ManifestValidationError::new(
                    "version prerelease is not valid SemVer",
                ));
            }
            if identifier.bytes().all(|byte| byte.is_ascii_digit()) {
                Ok(PreReleaseIdentifier::Numeric(Decimal::parse(
                    identifier,
                    "version prerelease",
                    false,
                )?))
            } else {
                Ok(PreReleaseIdentifier::Text(identifier.to_string()))
            }
        })
        .collect()
}

fn parse_build_metadata(value: &str) -> Result<Vec<String>, ManifestValidationError> {
    if value.is_empty() {
        return Err(ManifestValidationError::new(
            "version build metadata must not be empty",
        ));
    }
    let identifiers: Vec<String> = value.split('.').map(str::to_string).collect();
    if identifiers.iter().any(|identifier| {
        identifier.is_empty()
            || !identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return Err(ManifestValidationError::new(
            "version build metadata is not valid SemVer",
        ));
    }
    Ok(identifiers)
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct MacOsVersion {
    components: Vec<Decimal>,
}

impl MacOsVersion {
    pub(crate) fn parse(value: &str, field: &str) -> Result<Self, ManifestValidationError> {
        validate_non_empty_text(value, field)?;
        let mut components: Vec<Decimal> = value
            .split('.')
            .map(|component| Decimal::parse(component, field, true))
            .collect::<Result<_, _>>()?;
        while components.len() > 1 && components.last().is_some_and(Decimal::is_zero) {
            components.pop();
        }
        Ok(Self { components })
    }

    fn compare_padded(&self, other: &Self) -> Ordering {
        let zero = Decimal::zero();
        let component_count = self.components.len().max(other.components.len());
        (0..component_count)
            .map(|index| {
                let left = self.components.get(index).unwrap_or(&zero);
                let right = other.components.get(index).unwrap_or(&zero);
                left.cmp(right)
            })
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    }
}

impl Ord for MacOsVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.compare_padded(other)
    }
}

impl PartialOrd for MacOsVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for MacOsVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, component) in self.components.iter().enumerate() {
            if index > 0 {
                formatter.write_str(".")?;
            }
            component.fmt(formatter)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawManifest {
    product_id: String,
    schema_version: u32,
    releases: Vec<RawRelease>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawRelease {
    channel: String,
    version: String,
    architecture: String,
    min_version: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    max_version_exclusive: Option<String>,
    artifact: RawArtifact,
    signature: RawSignature,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawArtifact {
    artifact_id: String,
    format: String,
    url: String,
    size_bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawSignature {
    key_id: String,
    proof: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct DuplicateTarget {
    channel: String,
    version: SemVer,
    architecture: String,
    min_version: MacOsVersion,
    max_version_exclusive: Option<MacOsVersion>,
}

fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

pub(crate) fn parse_manifest(input: &str) -> Result<UpdateManifest, ManifestValidationError> {
    let raw: RawManifest = serde_json::from_str(input).map_err(|error| {
        ManifestValidationError::new(format!("manifest JSON is invalid: {error}"))
    })?;
    normalize_manifest(raw)
}

fn normalize_manifest(raw: RawManifest) -> Result<UpdateManifest, ManifestValidationError> {
    let product_id = raw.product_id;
    let schema_version = raw.schema_version;
    if schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(ManifestValidationError::new(
            "manifest schema version is unsupported",
        ));
    }
    validate_non_empty_text(&product_id, "manifest productId")?;
    if product_id != MANIFEST_PRODUCT_ID {
        return Err(ManifestValidationError::new(
            "manifest productId is not supported",
        ));
    }

    let raw_releases = raw.releases;
    let mut seen_targets = std::collections::HashSet::new();
    let mut releases = Vec::with_capacity(raw_releases.len());
    for (index, raw_release) in raw_releases.into_iter().enumerate() {
        let release = normalize_release(raw_release, index)?;
        let target = DuplicateTarget {
            channel: release.channel.clone(),
            version: release.version.clone(),
            architecture: release.architecture.clone(),
            min_version: release.min_version.clone(),
            max_version_exclusive: release.max_version_exclusive.clone(),
        };
        if !seen_targets.insert(target) {
            return Err(ManifestValidationError::new(format!(
                "manifest contains duplicate release target at index {index}"
            )));
        }

        if release.channel == TARGET_CHANNEL
            && !release.version.is_prerelease()
            && release.architecture == TARGET_ARCHITECTURE
            && release.artifact.format == TARGET_ARTIFACT_FORMAT
        {
            releases.push(release);
        }
    }

    Ok(UpdateManifest {
        product_id,
        schema_version,
        releases,
    })
}

fn normalize_release(
    raw: RawRelease,
    index: usize,
) -> Result<UpdateRelease, ManifestValidationError> {
    validate_non_empty_text(&raw.channel, &format!("release {index} channel"))?;
    validate_non_empty_text(&raw.architecture, &format!("release {index} architecture"))?;
    let version = SemVer::parse(&raw.version).map_err(|error| {
        ManifestValidationError::new(format!("release {index} version is invalid: {error}"))
    })?;
    let min_version = MacOsVersion::parse(&raw.min_version, "release minVersion")?;
    let max_version_exclusive = match raw.max_version_exclusive {
        None => None,
        Some(value) => Some(MacOsVersion::parse(&value, "release maxVersionExclusive")?),
    };
    if let Some(max_version_exclusive) = &max_version_exclusive {
        if max_version_exclusive <= &min_version {
            return Err(ManifestValidationError::new(
                "release macOS version range is empty or reversed",
            ));
        }
    }

    let artifact = normalize_artifact(raw.artifact, index)?;
    let signature = normalize_signature(raw.signature, index)?;

    Ok(UpdateRelease {
        channel: raw.channel,
        version,
        architecture: raw.architecture,
        min_version,
        max_version_exclusive,
        artifact,
        signature,
    })
}

fn normalize_artifact(
    raw: RawArtifact,
    index: usize,
) -> Result<UpdateArtifact, ManifestValidationError> {
    validate_non_empty_text(&raw.artifact_id, &format!("release {index} artifactId"))?;
    validate_non_empty_text(&raw.format, &format!("release {index} artifact format"))?;
    if raw.size_bytes == 0 {
        return Err(ManifestValidationError::new(format!(
            "release {index} artifact sizeBytes must be positive"
        )));
    }
    if raw.sha256.len() != 64
        || !raw
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ManifestValidationError::new(format!(
            "release {index} artifact sha256 must be lowercase hexadecimal SHA-256"
        )));
    }

    Ok(UpdateArtifact {
        artifact_id: raw.artifact_id,
        format: raw.format,
        url: HttpsUrl::parse(raw.url)?,
        size_bytes: raw.size_bytes,
        sha256: raw.sha256,
    })
}

fn normalize_signature(
    raw: RawSignature,
    index: usize,
) -> Result<UpdateSignature, ManifestValidationError> {
    validate_non_empty_text(&raw.key_id, &format!("release {index} signature keyId"))?;
    validate_non_empty_text(&raw.proof, &format!("release {index} signature proof"))?;
    Ok(UpdateSignature {
        key_id: raw.key_id,
        proof: raw.proof,
    })
}

fn validate_non_empty_text(value: &str, field: &str) -> Result<(), ManifestValidationError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(ManifestValidationError::new(format!(
            "{field} must be a non-empty string without control characters"
        )));
    }
    Ok(())
}

fn authority_contains_userinfo(value: &str) -> bool {
    let Some(scheme_end) = value.find("://") else {
        return false;
    };
    let authority_start = scheme_end + 3;
    let authority_end = value[authority_start..]
        .find(|character: char| matches!(character, '/' | '?' | '#'))
        .map_or(value.len(), |offset| authority_start + offset);
    value[authority_start..authority_end].contains('@')
}

pub(crate) fn is_safe_public_update_url(url: &Url) -> bool {
    url.scheme().eq_ignore_ascii_case("https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && !authority_contains_userinfo(url.as_str())
        && url.fragment().is_none()
        && url.query().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_RELEASE: &str = r#"
        {
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
            "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
          },
          "signature": { "keyId": "current-key", "proof": "opaque-proof" }
        }
    "#;

    fn manifest_with_releases(releases: &str) -> String {
        format!(
            r#"{{"productId":"{MANIFEST_PRODUCT_ID}","schemaVersion":1,"releases":[{releases}]}}"#
        )
    }

    fn valid_manifest() -> String {
        manifest_with_releases(VALID_RELEASE)
    }

    fn manifest_with_artifact_url(url: &str) -> String {
        valid_manifest().replace("https://updates.example.test/cornell-method/app", url)
    }

    fn assert_invalid(json: &str) {
        assert!(
            parse_manifest(json).is_err(),
            "expected invalid manifest: {json}"
        );
    }

    #[test]
    fn normalizes_valid_stable_apple_silicon_release_without_selecting_a_version() {
        let manifest = parse_manifest(&valid_manifest()).unwrap();
        assert_eq!(manifest.product_id, MANIFEST_PRODUCT_ID);
        assert_eq!(manifest.schema_version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(manifest.releases.len(), 1);
        let release = &manifest.releases[0];
        assert_eq!(release.version.to_string(), "1.2.3+build.7");
        assert_eq!(release.min_version.to_string(), "14");
        assert_eq!(
            release.max_version_exclusive.as_ref().unwrap().to_string(),
            "15"
        );
        assert_eq!(
            release.artifact.url.as_str(),
            "https://updates.example.test/cornell-method/app"
        );
        assert_eq!(release.artifact.size_bytes, 12345);
        assert_eq!(release.signature.proof, "opaque-proof");

        let without_max =
            parse_manifest(&valid_manifest().replace("\"maxVersionExclusive\": \"15.0\",\n", ""))
                .unwrap();
        assert!(without_max.releases[0].max_version_exclusive.is_none());
    }

    #[test]
    fn accepts_empty_releases_and_non_target_releases_as_no_update() {
        let empty = parse_manifest(&manifest_with_releases("")).unwrap();
        assert!(empty.is_no_update());

        for source in [
            valid_manifest().replace("\"channel\": \"stable\"", "\"channel\": \"beta\""),
            valid_manifest().replace(
                "\"architecture\": \"aarch64-apple-darwin\"",
                "\"architecture\": \"x86_64-apple-darwin\"",
            ),
            valid_manifest().replace("\"format\": \"app-archive\"", "\"format\": \"other\""),
        ] {
            assert!(parse_manifest(&source).unwrap().is_no_update());
        }

        let non_target_release = format!(
            r#"
            {{
              "channel":"beta",
              "version":"9.0.0",
              "architecture":"x86_64-apple-darwin",
              "minVersion":"14",
              "artifact":{{
                "artifactId":"beta-artifact",
                "format":"other-format",
                "url":"https://updates.example.test/beta",
                "sizeBytes":1,
                "sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
              }},
              "signature":{{"keyId":"beta-key","proof":"opaque"}}
            }}"#
        );
        let non_target_only = parse_manifest(&manifest_with_releases(&non_target_release)).unwrap();
        assert!(non_target_only.is_no_update());

        let manifest = parse_manifest(&manifest_with_releases(&format!(
            "{VALID_RELEASE},{non_target_release}"
        )))
        .unwrap();
        assert_eq!(manifest.releases.len(), 1);
        assert_eq!(manifest.releases[0].version.to_string(), "1.2.3+build.7");
    }

    #[test]
    fn excludes_stable_prereleases_but_keeps_valid_stable_releases() {
        let prerelease = VALID_RELEASE.replace(
            "\"version\": \"1.2.3+build.7\"",
            "\"version\": \"9.0.0-rc.1\"",
        );
        let prerelease_only = parse_manifest(&manifest_with_releases(&prerelease)).unwrap();
        assert!(prerelease_only.is_no_update());

        let mixed = parse_manifest(&manifest_with_releases(&format!(
            "{prerelease},{VALID_RELEASE}"
        )))
        .unwrap();
        assert_eq!(mixed.releases.len(), 1);
        assert_eq!(mixed.releases[0].version.to_string(), "1.2.3+build.7");
    }

    #[test]
    fn rejects_missing_required_fields_and_explicit_null_optional_version() {
        for source in [
            r#"{"schemaVersion":1,"releases":[]}"#,
            r#"{"productId":"com.cornellmethod.notebook","releases":[]}"#,
            r#"{"productId":"com.cornellmethod.notebook","schemaVersion":1}"#,
        ] {
            assert_invalid(source);
        }

        for field in [
            "\"channel\": \"stable\",\n",
            "\"version\": \"1.2.3+build.7\",\n",
            "\"architecture\": \"aarch64-apple-darwin\",\n",
            "\"minVersion\": \"14.0\",\n",
            "\"artifact\": {\n",
            "\"format\": \"app-archive\",\n",
            "\"signature\": { \"keyId\": \"current-key\", \"proof\": \"opaque-proof\" }\n",
        ] {
            assert_invalid(&valid_manifest().replace(field, ""));
        }

        assert_invalid(&valid_manifest().replace(
            "\"maxVersionExclusive\": \"15.0\",",
            "\"maxVersionExclusive\": null,",
        ));
        assert_invalid(
            &valid_manifest().replace("\"artifactId\": \"com.cornellmethod.notebook-1.2.3\",", ""),
        );
        assert_invalid(&valid_manifest().replace(
            "\"artifactId\": \"com.cornellmethod.notebook-1.2.3\"",
            "\"artifactId\": \"\"",
        ));
        assert_invalid(&valid_manifest().replace(
            "\"url\": \"https://updates.example.test/cornell-method/app\",",
            "",
        ));
        assert_invalid(&valid_manifest().replace("\"sizeBytes\": 12345,", ""));
        assert_invalid(&valid_manifest().replace(
            "\"sha256\": \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"",
            "",
        ));
    }

    #[test]
    fn rejects_root_and_nested_unknown_fields() {
        for (needle, replacement) in [
            ("\"releases\":[", "\"unexpected\":true,\"releases\":["),
            (
                "\"signature\": { \"keyId\": \"current-key\", \"proof\": \"opaque-proof\" }",
                "\"signature\": { \"keyId\": \"current-key\", \"proof\": \"opaque-proof\", \"extra\": true }",
            ),
        ] {
            let source = valid_manifest().replace(needle, replacement);
            assert_invalid(&source);
        }

        let release_unknown = valid_manifest().replace(
            "\"architecture\": \"aarch64-apple-darwin\",",
            "\"architecture\": \"aarch64-apple-darwin\",\"extra\":true,",
        );
        assert_invalid(&release_unknown);

        let artifact_unknown = valid_manifest().replace(
            "\"format\": \"app-archive\",",
            "\"format\": \"app-archive\",\"extra\":true,",
        );
        assert_invalid(&artifact_unknown);
    }

    #[test]
    fn rejects_product_schema_and_malformed_semver() {
        assert_invalid(&valid_manifest().replace(MANIFEST_PRODUCT_ID, "com.other.product"));
        assert_invalid(&valid_manifest().replace("\"schemaVersion\":1", "\"schemaVersion\":2"));
        assert_invalid(&valid_manifest().replace("\"schemaVersion\":1", "\"schemaVersion\":1.0"));
        assert_invalid(
            &valid_manifest().replace("\"version\": \"1.2.3+build.7\"", "\"version\": \"1.2\""),
        );
        assert_invalid(
            &valid_manifest().replace("\"version\": \"1.2.3+build.7\"", "\"version\": \"\""),
        );
        assert_invalid(&valid_manifest().replace(
            "\"version\": \"1.2.3+build.7\"",
            "\"version\": \"1.2.3\\u0000\"",
        ));
    }

    #[test]
    fn rejects_invalid_macos_ranges_and_artifact_metadata() {
        for replacement in [
            ("\"minVersion\": \"14.0\"", "\"minVersion\": \"14.x\""),
            (
                "\"maxVersionExclusive\": \"15.0\"",
                "\"maxVersionExclusive\": \"14.0\"",
            ),
            (
                "\"maxVersionExclusive\": \"15.0\"",
                "\"maxVersionExclusive\": \"14.0.0\"",
            ),
            ("\"sizeBytes\": 12345", "\"sizeBytes\": 0"),
            ("\"sizeBytes\": 12345", "\"sizeBytes\": 1.5"),
            (
                "\"sha256\": \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"",
                "\"sha256\": \"0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef\"",
            ),
            (
                "\"sha256\": \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"",
                "\"sha256\": \"short\"",
            ),
            (
                "\"url\": \"https://updates.example.test/cornell-method/app\"",
                "\"url\": \"http://updates.example.test/cornell-method/app\"",
            ),
            (
                "\"url\": \"https://updates.example.test/cornell-method/app\"",
                "\"url\": \"https://user:pass@updates.example.test/app\"",
            ),
            (
                "\"url\": \"https://updates.example.test/cornell-method/app\"",
                "\"url\": \"https://updates.example.test/app?access_token=secret\"",
            ),
        ] {
            assert_invalid(&valid_manifest().replace(replacement.0, replacement.1));
        }
    }

    #[test]
    fn rejects_any_artifact_url_query_or_fragment() {
        for url in [
            "https://updates.example.test/cornell-method/app?sig=opaque-signature",
            "https://updates.example.test/cornell-method/app?signature=opaque-signature",
            "https://updates.example.test/cornell-method/app?X-Amz-Signature=opaque-signature",
            "https://updates.example.test/cornell-method/app?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=release%2F20260824%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20260824T000000Z&X-Amz-Expires=300&X-Amz-SignedHeaders=host&X-Amz-Signature=opaque-signature",
            "https://updates.example.test/cornell-method/app?access_token=opaque-token",
            "https://updates.example.test/cornell-method/app?opaque=provider-value",
            "https://updates.example.test/cornell-method/app?opaque",
            "https://updates.example.test/cornell-method/app?%73ig=opaque-signature",
            "https://updates.example.test/cornell-method/app?",
            "https://updates.example.test/cornell-method/app#fragment",
        ] {
            assert_invalid(&manifest_with_artifact_url(url));
        }
    }

    #[test]
    fn rejects_missing_empty_or_typed_signature_values() {
        for replacement in [
            ("\"keyId\": \"current-key\"", "\"keyId\": \"\""),
            ("\"proof\": \"opaque-proof\"", "\"proof\": \"\""),
            ("\"proof\": \"opaque-proof\"", "\"proof\": 42"),
            (
                "\"signature\": { \"keyId\": \"current-key\", \"proof\": \"opaque-proof\" }",
                "\"signature\": { \"keyId\": \"current-key\" }",
            ),
        ] {
            assert_invalid(&valid_manifest().replace(replacement.0, replacement.1));
        }
    }

    #[test]
    fn rejects_duplicate_targets_before_non_target_filtering() {
        let duplicate = manifest_with_releases(&format!("{VALID_RELEASE},{VALID_RELEASE}"));
        assert_invalid(&duplicate);

        let duplicate_non_target = manifest_with_releases(&format!(
            r#"{{
              "channel":"beta",
              "version":"2.0.0",
              "architecture":"x86_64-apple-darwin",
              "minVersion":"14.0",
              "artifact":{{"artifactId":"beta-1","format":"other","url":"https://updates.example.test/beta-1","sizeBytes":1,"sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}},
              "signature":{{"keyId":"beta-key","proof":"opaque"}}
            }},
            {{
              "channel":"beta",
              "version":"2.0.0+different-build",
              "architecture":"x86_64-apple-darwin",
              "minVersion":"14",
              "artifact":{{"artifactId":"beta-2","format":"other","url":"https://updates.example.test/beta-2","sizeBytes":1,"sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}},
              "signature":{{"keyId":"beta-key","proof":"opaque"}}
            }}"#
        ));
        assert_invalid(&duplicate_non_target);
    }

    #[test]
    fn semver_precedence_ignores_build_metadata_and_uses_semver_order() {
        let one = SemVer::parse("1.0.0+one").unwrap();
        let two = SemVer::parse("1.0.0+two").unwrap();
        let alpha = SemVer::parse("1.0.0-alpha.10").unwrap();
        let beta = SemVer::parse("1.0.0-beta.2").unwrap();
        assert_eq!(one, two);
        assert!(one < SemVer::parse("1.0.1").unwrap());
        assert!(alpha < beta);
        assert!(beta < one);
    }
}
