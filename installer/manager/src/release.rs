use crate::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};

pub const MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024;
const MAX_UNPACKED: u64 = 4 * MAX_DOWNLOAD;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub version: String,
    pub update_url: Option<String>,
    pub platforms: BTreeMap<String, Asset>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub version: String,
    pub manifest: String,
}

pub fn target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        _ => Err("This operating system/architecture does not have an installer yet".into()),
    }
}
fn version(value: &str) -> Result<()> {
    let parsed = semver::Version::parse(value)?;
    if parsed.to_string() != value || value.contains(['/', '\\']) {
        return Err("Invalid version".into());
    }
    Ok(())
}
fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(900))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?)
}
fn remote(source: &str) -> bool {
    source.starts_with("https://")
}
impl Manifest {
    pub fn read(source: &str) -> Result<Self> {
        let data = if remote(source) {
            let mut data = Vec::new();
            client()?
                .get(source)
                .send()?
                .error_for_status()?
                .take(1024 * 1024 + 1)
                .read_to_end(&mut data)?;
            data
        } else {
            if source.contains("://") {
                return Err("Release URLs must use HTTPS".into());
            }
            let file = fs::File::open(source)?;
            let mut data = Vec::new();
            file.take(1024 * 1024 + 1).read_to_end(&mut data)?;
            data
        };
        if data.len() > 1024 * 1024 {
            return Err("Release manifest is too large".into());
        }
        let manifest: Self = serde_json::from_slice(&data)?;
        if manifest.schema_version != 1 {
            return Err("Unsupported release manifest schema; rerun the latest installer".into());
        }
        version(&manifest.version)?;
        if let Some(url) = &manifest.update_url
            && (url.is_empty()
                || (remote(source) && !remote(url))
                || (!remote(url) && url.contains("://")))
        {
            return Err("Update channel must use HTTPS".into());
        }
        for asset in manifest.platforms.values() {
            if asset.sha256.len() != 64
                || !asset.sha256.bytes().all(|b| b.is_ascii_hexdigit())
                || asset.size == 0
                || asset.size > MAX_DOWNLOAD
            {
                return Err("Invalid release checksum or size".into());
            }
            if remote(source) && !remote(&asset.url) {
                return Err("A remote manifest must use HTTPS artifact URLs".into());
            }
        }
        Ok(manifest)
    }
    pub fn asset(&self) -> Result<&Asset> {
        self.platforms
            .get(target()?)
            .ok_or_else(|| "This release has no download for this platform".into())
    }
}
impl State {
    pub fn read(prefix: &Path) -> Result<Option<Self>> {
        let file = match fs::read(prefix.join("current.json")) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let state: Self = serde_json::from_slice(&file)?;
        version(&state.version)?;
        Ok(Some(state))
    }
    pub fn write(&self, prefix: &Path) -> Result<()> {
        version(&self.version)?;
        let mut pending = tempfile::NamedTempFile::new_in(prefix)?;
        pending.write_all(&serde_json::to_vec_pretty(self)?)?;
        pending.as_file().sync_all()?;
        // Tempfile uses replace semantics on Windows as well as Unix.
        pending
            .persist(prefix.join("current.json"))
            .map_err(|e| e.error)?;
        Ok(())
    }
}
fn safe_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|c| matches!(c, Component::Normal(_)))
        && !path.to_string_lossy().contains(['\\', ':'])
}
fn unpack(archive: &Path, destination: &Path, zip: bool) -> Result<()> {
    let mut total = 0_u64;
    if zip {
        let mut archive = zip::ZipArchive::new(fs::File::open(archive)?)?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let path = entry.enclosed_name().ok_or("Unsafe ZIP path")?;
            if !safe_path(&path) || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                return Err("Unsafe ZIP entry".into());
            }
            total = total
                .checked_add(entry.size())
                .ok_or("Archive size overflow")?;
            if total > MAX_UNPACKED {
                return Err("Unpacked release is too large".into());
            }
            let target = destination.join(path);
            if entry.is_dir() {
                fs::create_dir_all(target)?;
            } else {
                fs::create_dir_all(target.parent().ok_or("Missing archive parent")?)?;
                let mut file = fs::File::create(target)?;
                std::io::copy(&mut entry, &mut file)?;
            }
        }
    } else {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(archive)?));
        for entry in archive.entries()? {
            let mut entry = entry?;
            if !safe_path(&entry.path()?)
                || !(entry.header().entry_type().is_file() || entry.header().entry_type().is_dir())
            {
                return Err("Unsafe TAR entry (links and special files are not allowed)".into());
            }
            total = total
                .checked_add(entry.size())
                .ok_or("Archive size overflow")?;
            if total > MAX_UNPACKED {
                return Err("Unpacked release is too large".into());
            }
            if !entry.unpack_in(destination)? {
                return Err("Archive path escapes destination".into());
            }
        }
    }
    Ok(())
}
pub fn stage(prefix: &Path, version: &str, asset: &Asset, source: &str) -> Result<PathBuf> {
    let versions = prefix.join("versions");
    fs::create_dir_all(&versions)?;
    let destination = versions.join(version);
    // Never reuse an unverified directory or overwrite an active/running version.
    if destination.exists() {
        return Err(format!(
            "{} already exists; remove that inactive directory before retrying",
            destination.display()
        )
        .into());
    }
    let staging = tempfile::tempdir_in(&versions)?;
    let mut archive = tempfile::NamedTempFile::new_in(prefix)?;
    let mut reader: Box<dyn Read> = if remote(&asset.url) {
        Box::new(client()?.get(&asset.url).send()?.error_for_status()?)
    } else {
        if remote(source) || asset.url.contains("://") {
            return Err("Artifact URLs must use HTTPS".into());
        }
        let path = Path::new(source)
            .parent()
            .unwrap_or(Path::new("."))
            .join(&asset.url);
        Box::new(fs::File::open(path)?)
    };
    let mut hash = Sha256::new();
    let mut downloaded = 0_u64;
    let mut buffer = [0_u8; 65536];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        downloaded += count as u64;
        if downloaded > asset.size {
            return Err("Download exceeds the advertised size".into());
        }
        hash.update(&buffer[..count]);
        archive.write_all(&buffer[..count])?;
    }
    if downloaded != asset.size
        || format!("{:x}", hash.finalize()) != asset.sha256.to_ascii_lowercase()
    {
        return Err("Release size or SHA-256 checksum mismatch".into());
    }
    archive.flush()?;
    unpack(
        archive.path(),
        staging.path(),
        target()?.contains("windows"),
    )?;
    if !staging
        .path()
        .join("bin")
        .join(crate::platform::compiler_name())
        .is_file()
    {
        return Err("Release does not contain the compiler".into());
    }
    fs::rename(staging.path(), &destination)?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_and_versions_cannot_escape_the_installation() {
        for bad in [
            "../outside",
            "/tmp/a",
            "C:\\outside",
            "file:stream",
            "a/../b",
            "",
        ] {
            assert!(!safe_path(Path::new(bad)), "{bad}");
        }
        assert!(safe_path(Path::new("lib/libLLVM.dylib")));
        assert!(version("../../bad").is_err());
        assert!(version("0.1.1").is_ok());
    }
    #[test]
    fn state_replace_and_corruption_handling() {
        let dir = tempfile::tempdir().unwrap();
        assert!(State::read(dir.path()).unwrap().is_none());
        for v in ["0.1.0", "0.2.0"] {
            State {
                version: v.into(),
                manifest: "https://example.com/latest.json".into(),
            }
            .write(dir.path())
            .unwrap();
        }
        assert_eq!(State::read(dir.path()).unwrap().unwrap().version, "0.2.0");
        fs::write(dir.path().join("current.json"), "{}").unwrap();
        assert!(State::read(dir.path()).is_err());
    }
    #[test]
    fn bad_checksum_does_not_publish_a_version() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("install");
        fs::create_dir_all(&prefix).unwrap();
        fs::write(dir.path().join("payload"), b"bad").unwrap();
        let asset = Asset {
            url: "payload".into(),
            sha256: "0".repeat(64),
            size: 3,
        };
        assert!(
            stage(
                &prefix,
                "0.1.0",
                &asset,
                dir.path().join("manifest.json").to_str().unwrap()
            )
            .is_err()
        );
        assert!(!prefix.join("versions/0.1.0").exists());
    }
    #[test]
    fn manifests_validate_schema_versions_sizes_and_update_channels() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("release.json");
        let valid = serde_json::json!({"schema_version": 1, "version": "0.1.0",
            "platforms": {target().unwrap(): {"url": "payload.tar.gz", "sha256": "a".repeat(64), "size": 10}}});
        fs::write(&path, valid.to_string()).unwrap();
        assert!(
            Manifest::read(path.to_str().unwrap())
                .unwrap()
                .asset()
                .is_ok()
        );
        for (key, value) in [
            ("schema_version", serde_json::json!(2)),
            ("version", serde_json::json!("../bad")),
            (
                "update_url",
                serde_json::json!("http://example.org/latest.json"),
            ),
        ] {
            let mut bad = valid.clone();
            bad[key] = value;
            fs::write(&path, bad.to_string()).unwrap();
            assert!(Manifest::read(path.to_str().unwrap()).is_err());
        }
        assert!(Manifest::read("http://example.org/latest.json").is_err());
    }
    #[test]
    fn archive_links_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bad.tar.gz");
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            fs::File::create(&file).unwrap(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        archive
            .append_link(&mut header, "escape", "../outside")
            .unwrap();
        archive.into_inner().unwrap().finish().unwrap();
        assert!(unpack(&file, dir.path(), false).is_err());
    }
}
