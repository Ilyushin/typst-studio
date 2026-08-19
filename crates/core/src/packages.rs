//! The Typst Universe package index, used for import completion.

use std::path::PathBuf;
use std::time::Duration;

use ecow::EcoString;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use typst::syntax::package::{PackageSpec, PackageVersion};
use typst_kit::downloader::{Downloader, SystemDownloader};

/// The namespace Universe packages live in.
const NAMESPACE: &str = "preview";

/// Where the index is published.
const INDEX_URL: &str = "https://packages.typst.org/preview/index.json";

/// How long a cached index is served before being fetched again.
///
/// The index is two megabytes; re-downloading it on every start would be rude
/// on a metered connection, and a day-old list of packages is no worse for
/// completion.
const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// One package as the registry describes it.
///
/// The index carries more fields than this; unknown ones are ignored so a
/// registry change does not break completion.
#[derive(Deserialize)]
struct Entry {
    name: EcoString,
    version: PackageVersion,
    description: Option<EcoString>,
}

/// Downloads the package index.
///
/// Blocking, and roughly two megabytes over the network — call it off the UI
/// thread.
pub fn fetch_package_index() -> Result<Vec<(PackageSpec, Option<EcoString>)>, String> {
    if let Some(data) = read_cache() {
        return parse_package_index(&data);
    }

    let downloader = SystemDownloader::new(concat!(
        "typst-studio/",
        env!("CARGO_PKG_VERSION")
    ));

    let data = downloader
        .download(&"package index", INDEX_URL)
        .map_err(|err| format!("could not download the package index: {err}"))?;

    let packages = parse_package_index(&data)?;
    write_cache(&data);
    Ok(packages)
}

/// Where the downloaded index is kept between runs.
fn cache_path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("typst-studio").join("package-index.json"))
}

/// Reads the cached index, unless it is missing or too old.
fn read_cache() -> Option<Vec<u8>> {
    let path = cache_path()?;
    let age = path.metadata().ok()?.modified().ok()?.elapsed().ok()?;
    (age < CACHE_MAX_AGE).then(|| std::fs::read(path).ok())?
}

/// Stores the index for the next run. Failure is not worth reporting: the only
/// cost is downloading it again.
fn write_cache(data: &[u8]) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
    {
        let _ = std::fs::write(path, data);
    }
}

/// Discards the cached index, so the next fetch goes to the network.
pub fn clear_package_cache() {
    if let Some(path) = cache_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Parses the index, keeping the newest version of each package.
pub fn parse_package_index(
    data: &[u8],
) -> Result<Vec<(PackageSpec, Option<EcoString>)>, String> {
    // Entries are parsed individually so that one the current compiler cannot
    // understand is skipped rather than failing the whole index.
    let values: Vec<serde_json::Value> = serde_json::from_slice(data)
        .map_err(|err| format!("could not parse the package index: {err}"))?;

    let mut newest: FxHashMap<EcoString, Entry> = FxHashMap::default();
    for value in values {
        let Ok(entry) = Entry::deserialize(value) else { continue };
        match newest.get(&entry.name) {
            Some(existing) if existing.version >= entry.version => {}
            _ => {
                newest.insert(entry.name.clone(), entry);
            }
        }
    }

    let mut packages: Vec<_> = newest
        .into_values()
        .map(|entry| {
            let spec = PackageSpec {
                namespace: NAMESPACE.into(),
                name: entry.name,
                version: entry.version,
            };
            (spec, entry.description)
        })
        .collect();

    packages.sort_by(|(a, _), (b, _)| a.name.cmp(&b.name));
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
        {"name": "cetz", "version": "0.2.0", "description": "Drawing"},
        {"name": "cetz", "version": "0.3.1", "description": "Drawing, newer"},
        {"name": "polylux", "version": "0.4.0"},
        {"name": "broken", "version": "not-a-version"}
    ]"#;

    #[test]
    fn keeps_the_newest_version_of_each_package() {
        let packages = parse_package_index(SAMPLE.as_bytes()).unwrap();

        let names: Vec<_> = packages.iter().map(|(spec, _)| spec.name.as_str()).collect();
        assert_eq!(names, ["cetz", "polylux"], "unparsable entries are skipped");

        let (cetz, description) = &packages[0];
        assert_eq!(cetz.version.to_string(), "0.3.1");
        assert_eq!(cetz.namespace, "preview");
        assert_eq!(description.as_deref(), Some("Drawing, newer"));

        assert_eq!(packages[1].1, None, "a missing description is allowed");
    }

    #[test]
    fn rejects_malformed_index() {
        let error = parse_package_index(b"not json").unwrap_err();
        assert!(error.contains("could not parse"), "got: {error}");
    }
}
