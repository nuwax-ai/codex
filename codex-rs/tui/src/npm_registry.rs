use serde::Deserialize;
use std::collections::HashMap;

#[cfg(not(debug_assertions))]
pub(crate) const PACKAGE_URL: &str = "https://registry.npmjs.org/@openai%2fcodex";

/// Fork channel: `nuwax-codex` on npm. Unlike the OpenAI flow there is no
/// GitHub-release cross-check — this registry IS the authoritative source.
#[cfg(not(debug_assertions))]
pub(crate) const NUWAX_PACKAGE_URL: &str = "https://registry.npmjs.org/nuwax-codex";

#[derive(Deserialize, Debug, Clone)]
pub(crate) struct NpmPackageInfo {
    #[serde(rename = "dist-tags")]
    dist_tags: HashMap<String, String>,
    versions: HashMap<String, NpmPackageVersionInfo>,
}

#[derive(Deserialize, Debug, Clone)]
struct NpmPackageVersionInfo {
    dist: Option<NpmPackageDist>,
}

#[derive(Deserialize, Debug, Clone)]
struct NpmPackageDist {
    tarball: Option<String>,
    integrity: Option<String>,
}

pub(crate) fn ensure_version_ready(
    package_info: &NpmPackageInfo,
    version: &str,
) -> anyhow::Result<()> {
    let version = version.trim();

    match package_info.dist_tags.get("latest").map(String::as_str) {
        Some(latest) if latest == version => {}
        Some(latest) => anyhow::bail!(
            "npm latest dist-tag points to {latest}, expected GitHub release {version}"
        ),
        None => anyhow::bail!("npm package is missing latest dist-tag"),
    }

    version_info_with_dist(package_info, version)?;
    Ok(())
}

/// The package's `latest` dist-tag — the fork's update channel resolves the
/// newest version directly from npm instead of cross-checking GitHub.
pub(crate) fn latest_version(package_info: &NpmPackageInfo) -> anyhow::Result<String> {
    let latest = package_info
        .dist_tags
        .get("latest")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("npm package is missing latest dist-tag"))?;
    version_info_with_dist(package_info, &latest)?;
    Ok(latest)
}

fn version_info_with_dist<'a>(
    package_info: &'a NpmPackageInfo,
    version: &str,
) -> anyhow::Result<&'a NpmPackageVersionInfo> {
    let info = package_info
        .versions
        .get(version)
        .ok_or_else(|| anyhow::anyhow!("npm package version {version} is missing"))?;
    let Some(dist) = info.dist.as_ref() else {
        anyhow::bail!("npm package version {version} is missing dist metadata");
    };
    let has_tarball = dist
        .tarball
        .as_deref()
        .is_some_and(|tarball| !tarball.is_empty());
    if !has_tarball {
        anyhow::bail!("npm package version {version} is missing dist.tarball");
    }
    let has_integrity = dist
        .integrity
        .as_ref()
        .is_some_and(|integrity| !integrity.is_empty());
    if !has_integrity {
        anyhow::bail!("npm package version {version} is missing dist.integrity");
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version_json(version: &str) -> serde_json::Value {
        serde_json::json!({
            "dist": {
                "integrity": format!("sha512-{version}"),
                "tarball": format!("https://registry.npmjs.org/@openai/codex/-/codex-{version}.tgz"),
            }
        })
    }

    fn package_info(github_latest: &str, npm_latest: &str) -> NpmPackageInfo {
        let mut versions = serde_json::Map::new();
        versions.insert(github_latest.to_string(), version_json(github_latest));

        serde_json::from_value(serde_json::json!({
            "dist-tags": { "latest": npm_latest },
            "versions": serde_json::Value::Object(versions),
        }))
        .expect("valid npm package metadata")
    }

    #[test]
    fn ready_version_requires_latest_dist_tag_and_root_dist() {
        let latest = "1.2.3";
        let package_info = package_info(latest, latest);

        ensure_version_ready(&package_info, latest).expect("npm package is ready");
    }

    #[test]
    fn ready_version_rejects_stale_latest_dist_tag() {
        let package_info = package_info("1.2.3", "1.2.2");

        let err = ensure_version_ready(&package_info, "1.2.3")
            .expect_err("npm latest dist-tag must match GitHub latest");
        assert!(
            err.to_string().contains("latest dist-tag"),
            "error should name stale latest dist-tag: {err}"
        );
    }

    #[test]
    fn ready_version_rejects_missing_root_dist() {
        let package_info: NpmPackageInfo = serde_json::from_value(serde_json::json!({
            "dist-tags": { "latest": "1.2.3" },
            "versions": { "1.2.3": {} },
        }))
        .expect("valid npm package metadata");

        let err = ensure_version_ready(&package_info, "1.2.3")
            .expect_err("root package must have dist metadata");
        assert!(
            err.to_string().contains("missing dist metadata"),
            "error should name missing dist metadata: {err}"
        );
    }

    #[test]
    fn latest_version_returns_dist_tag_and_checks_dist() {
        let package_info = package_info("1.2.3", "1.2.3");
        assert_eq!(
            latest_version(&package_info).expect("latest resolvable"),
            "1.2.3"
        );

        let broken: NpmPackageInfo = serde_json::from_value(serde_json::json!({
            "dist-tags": { "latest": "1.2.3" },
            "versions": { "1.2.3": {} },
        }))
        .expect("valid npm package metadata");
        assert!(
            latest_version(&broken).is_err(),
            "latest without dist metadata must not become an update target"
        );
    }
}
