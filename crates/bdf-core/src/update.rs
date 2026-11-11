//! Updates from GitHub releases, checked before anything is replaced.
//!
//! Every release carries `SHA256SUMS.txt` and `SHA256SUMS.txt.sig`, an
//! Ed25519 signature made by the release workflow with a key nobody else
//! holds. An update is accepted only when:
//!
//! 1. the release is newer than this program (never a downgrade),
//! 2. the signature verifies against [`PUBLIC_KEY_HEX`], built into BDF,
//! 3. the zip's SHA-256 is the one in that signed list, and
//! 4. the new program, written next to the old one, passes `--self-test`.
//!
//! Only then is the old program renamed aside (kept as `bdf.old`, so it
//! can be put back by hand) and the new one moved into its place. A
//! download that is cut off, tampered with or simply broken changes nothing.
//!
//! The network itself is the caller's: [`download`] takes a `fetch`
//! function, which keeps this module testable without one.

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

/// Where updates come from: the public repository.
pub const LATEST_API: &str = "https://api.github.com/repos/boode-hub/BDF/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/boode-hub/BDF/releases/latest";
/// Public half of the release signing key (Ed25519, 32 bytes). The release
/// workflow checks that its secret key matches this before it signs.
pub const PUBLIC_KEY_HEX: &str = "5b4c87db334ca8d9c66d55c1f6b67d4bf0b09b1d54029ebffc1b8e920ae8c1c2";
pub const SUMS: &str = "SHA256SUMS.txt";
pub const SIG: &str = "SHA256SUMS.txt.sig";
/// Downloads are refused, not truncated, beyond these sizes.
pub const MAX_ZIP: usize = 200 << 20;
pub const MAX_SMALL: usize = 1 << 20;
const MAX_PROGRAM: u64 = 300 << 20;

/// This build's name in release file names, if releases are made for it.
pub fn platform() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("win-x64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-x64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("mac-arm64")
    } else {
        None
    }
}

pub fn program_name() -> &'static str {
    if cfg!(windows) { "bdf.exe" } else { "bdf" }
}

pub fn public_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(&PUBLIC_KEY_HEX[2 * i..2 * i + 2], 16).unwrap_or(0);
    }
    k
}

/// `major.minor.patch`; pre-releases ("1.2.0-beta") are not versions here,
/// so they are never offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let mut it = s.split('.').map(|p| {
            (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse().ok())
                .flatten()
        });
        let v = Version(it.next()??, it.next()??, it.next()??);
        it.next().is_none().then_some(v)
    }

    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 0, 0))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    /// Release notes as written on GitHub (Markdown).
    pub notes: String,
    pub page: String,
    pub assets: Vec<Asset>,
}

impl Release {
    /// GitHub's "latest release" answer. Drafts and pre-releases are refused.
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_slice(json).map_err(|_| "GitHub sent something unexpected")?;
        if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
            return Err("the latest release is not a final release".into());
        }
        let tag = v["tag_name"].as_str().ok_or("GitHub sent no version")?;
        let version = Version::parse(tag).ok_or_else(|| format!("“{tag}” is not a version"))?;
        let assets = v["assets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|a| {
                        Some(Asset {
                            name: a["name"].as_str()?.to_string(),
                            url: a["browser_download_url"].as_str()?.to_string(),
                            size: a["size"].as_u64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Release {
            version,
            notes: v["body"].as_str().unwrap_or_default().to_string(),
            page: v["html_url"].as_str().unwrap_or(RELEASES_PAGE).to_string(),
            assets,
        })
    }

    pub fn zip_name(&self, platform: &str) -> String {
        format!("BDF-{}-{platform}.zip", self.version)
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }

    /// The download for this computer, if the release has one.
    pub fn zip_for(&self, platform: &str) -> Option<&Asset> {
        self.asset(&self.zip_name(platform))
    }
}

/// Checks the signature over the checksum list, then returns the SHA-256 it
/// lists for `name`.
pub fn signed_hash(
    sums: &[u8],
    sig: &[u8],
    name: &str,
    key: &[u8; 32],
) -> Result<[u8; 32], String> {
    let key = VerifyingKey::from_bytes(key).map_err(|_| "BDF's update key is invalid")?;
    let sig: [u8; 64] = sig
        .try_into()
        .map_err(|_| "the release signature is malformed")?;
    key.verify_strict(sums, &Signature::from_bytes(&sig))
        .map_err(|_| "the release is not signed with BDF's key")?;
    let text = std::str::from_utf8(sums).map_err(|_| "the signed checksum list is not text")?;
    for line in text.lines() {
        let Some((hash, file)) = line.trim_end().split_once(' ') else {
            continue;
        };
        // `sha256sum` writes "hash  name", or "hash *name" in binary mode.
        if file.trim_start_matches([' ', '*']) == name {
            return hex32(hash).ok_or_else(|| format!("the checksum for {name} is malformed"));
        }
    }
    Err(format!("{name} is not in the signed checksum list"))
}

fn hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Checks the zip against its signed checksum and takes the program out of
/// it: exactly one `bdf` / `bdf.exe`, at most one folder deep, of the right
/// kind for this platform.
pub fn unpack_program(zip: &[u8], expected: &[u8; 32]) -> Result<Vec<u8>, String> {
    if Sha256::digest(zip).as_slice() != expected {
        return Err("the download is damaged (its checksum does not match)".into());
    }
    let mut ar = zip::ZipArchive::new(Cursor::new(zip))
        .map_err(|e| format!("the zip is unreadable ({e})"))?;
    let name = program_name();
    let mut found = None;
    for i in 0..ar.len() {
        let f = ar
            .by_index(i)
            .map_err(|e| format!("the zip is unreadable ({e})"))?;
        let Some(path) = f.enclosed_name() else {
            continue;
        };
        if f.is_file()
            && path.file_name().is_some_and(|n| n == name)
            && path.components().count() <= 2
        {
            if found.is_some() {
                return Err(format!("the zip holds more than one {name}"));
            }
            found = Some(i);
        }
    }
    let i = found.ok_or_else(|| format!("the zip holds no {name}"))?;
    let f = ar
        .by_index(i)
        .map_err(|e| format!("the zip is unreadable ({e})"))?;
    if f.size() > MAX_PROGRAM {
        return Err("the program in the zip is implausibly large".into());
    }
    let mut out = Vec::with_capacity(f.size() as usize);
    f.take(MAX_PROGRAM + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("the zip is damaged ({e})"))?;
    if !looks_like_a_program(&out) {
        return Err(format!(
            "{name} in the zip is not a program for this computer"
        ));
    }
    Ok(out)
}

fn looks_like_a_program(b: &[u8]) -> bool {
    if cfg!(windows) {
        b.starts_with(b"MZ")
    } else if cfg!(target_os = "macos") {
        // 64-bit Mach-O, or a universal binary.
        b.starts_with(&[0xCF, 0xFA, 0xED, 0xFE]) || b.starts_with(&[0xCA, 0xFE, 0xBA, 0xBE])
    } else {
        b.starts_with(b"\x7fELF")
    }
}

/// Downloads one release file, refusing more than the given number of bytes.
pub type Fetch<'a> = dyn FnMut(&Asset, usize) -> Result<Vec<u8>, String> + 'a;

/// Downloads and checks an update: the small signed checksum list first, so a
/// release that is not BDF's is refused before the large download starts.
/// `fetch(asset, limit)` must fail rather than return more than `limit` bytes.
pub fn download(
    release: &Release,
    platform: &str,
    key: &[u8; 32],
    fetch: &mut Fetch<'_>,
) -> Result<Vec<u8>, String> {
    let zip_name = release.zip_name(platform);
    let need = |n: &str| {
        release
            .asset(n)
            .ok_or_else(|| format!("the release has no {n}"))
    };
    let (sums, sig, zip) = (need(SUMS)?, need(SIG)?, need(&zip_name)?);
    if zip.size as usize > MAX_ZIP {
        return Err("the download is implausibly large".into());
    }
    let hash = signed_hash(
        &fetch(sums, MAX_SMALL)?,
        &fetch(sig, MAX_SMALL)?,
        &zip_name,
        key,
    )?;
    unpack_program(&fetch(zip, MAX_ZIP)?, &hash)
}

/// `dir/bdf.<tag>[.exe]`, next to the program.
pub fn sibling(exe: &Path, tag: &str) -> PathBuf {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    exe.with_file_name(format!("bdf.{tag}{ext}"))
}

/// Puts `program` in place of `exe`. It is written next to it as `bdf.new`
/// and must pass `self_test` there before anything else changes; then `exe`
/// becomes `bdf.old` and `bdf.new` becomes `exe`. On any failure the old
/// program is left (or put back) exactly where it was.
pub fn install(
    exe: &Path,
    program: &[u8],
    self_test: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    let new = sibling(exe, "new");
    let old = sibling(exe, "old");
    let cannot = |e: std::io::Error| {
        format!(
            "BDF cannot replace itself in {} ({e}). Download the new version from the release page instead.",
            dir.display()
        )
    };
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&new)?;
        f.write_all(program)?;
        f.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&new);
        return Err(cannot(e));
    }
    if let Err(e) = self_test(&new) {
        let _ = std::fs::remove_file(&new);
        return Err(format!(
            "the new version failed its self-test ({e}); nothing was changed"
        ));
    }
    let _ = std::fs::remove_file(&old);
    // A running program can be renamed (not overwritten), even on Windows.
    if let Err(e) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(cannot(e));
    }
    if let Err(e) = std::fs::rename(&new, exe) {
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(&new);
        return Err(cannot(e));
    }
    Ok(())
}

/// Removes what an earlier update left next to `exe`. Called once the
/// program has started properly, so `bdf.old` stays as a way back until
/// the new version has proved itself.
pub fn clean_up(exe: &Path) {
    for tag in ["old", "new"] {
        let p = sibling(exe, tag);
        if p.exists() {
            match std::fs::remove_file(&p) {
                Ok(()) => log::info!("removed {} left by an update", p.display()),
                // Still running (just restarted): next time.
                Err(e) => log::debug!("could not remove {} yet: {e}", p.display()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const TEST_SEED: [u8; 32] = [7; 32];

    fn test_key() -> (SigningKey, [u8; 32]) {
        let sk = SigningKey::from_bytes(&TEST_SEED);
        let pk = sk.verifying_key().to_bytes();
        (sk, pk)
    }

    fn program() -> Vec<u8> {
        let magic: &[u8] = if cfg!(windows) {
            b"MZ"
        } else if cfg!(target_os = "macos") {
            &[0xCF, 0xFA, 0xED, 0xFE]
        } else {
            b"\x7fELF"
        };
        let mut p = magic.to_vec();
        p.extend((0..50_000u32).map(|i| (i * 31 % 251) as u8));
        p
    }

    /// A release zip laid out like `scripts/package.sh` makes it.
    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|b| format!("{b:02x}")).collect()
    }

    struct Fake {
        release: Release,
        files: std::collections::HashMap<String, Vec<u8>>,
    }

    /// A signed release of version 9.9.9 for this platform, served from memory.
    fn fake_release(sign_with: &SigningKey) -> Fake {
        let v = Version(9, 9, 9);
        let plat = platform().unwrap_or("win-x64");
        let folder = format!("BDF-{v}-{plat}");
        let zip = zip_of(&[
            (&format!("{folder}/{}", program_name()), &program()),
            (&format!("{folder}/README.txt"), b"hello"),
            (&format!("{folder}/licenses/PDFium-LICENSE.txt"), b"bsd"),
        ]);
        let zip_name = format!("{folder}.zip");
        let sums = format!(
            "{}  {zip_name}\n{}  BDF-{v}-other.zip\n",
            hex(&Sha256::digest(&zip)),
            "0".repeat(64)
        );
        let sig = sign_with.sign(sums.as_bytes()).to_bytes().to_vec();
        let mut files = std::collections::HashMap::new();
        files.insert(zip_name.clone(), zip);
        files.insert(SUMS.to_string(), sums.into_bytes());
        files.insert(SIG.to_string(), sig);
        let assets = files
            .iter()
            .map(|(n, b)| Asset {
                name: n.clone(),
                url: format!("mem://{n}"),
                size: b.len() as u64,
            })
            .collect();
        Fake {
            release: Release {
                version: v,
                notes: String::new(),
                page: RELEASES_PAGE.into(),
                assets,
            },
            files,
        }
    }

    fn run(fake: &Fake, key: &[u8; 32], log: &mut Vec<String>) -> Result<Vec<u8>, String> {
        let plat = platform().unwrap_or("win-x64");
        download(&fake.release, plat, key, &mut |a, limit| {
            log.push(a.name.clone());
            let b = fake.files.get(&a.name).cloned().ok_or("404")?;
            if b.len() > limit {
                return Err("too large".into());
            }
            Ok(b)
        })
    }

    #[test]
    fn versions() {
        assert_eq!(Version::parse("v0.1.3"), Some(Version(0, 1, 3)));
        assert_eq!(Version::parse("0.1.10"), Some(Version(0, 1, 10)));
        assert!(Version(0, 1, 10) > Version(0, 1, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "1.2.3-beta",
            "v",
            "1..3",
            "+1.2.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn github_answers_are_read_and_odd_ones_refused() {
        let json = br#"{"tag_name":"v0.2.0","body":"Notes","html_url":"https://x/r",
            "draft":false,"prerelease":false,
            "assets":[{"name":"SHA256SUMS.txt","browser_download_url":"https://x/s","size":268},
                      {"name":"broken"}]}"#;
        let r = Release::parse(json).unwrap();
        assert_eq!(
            (r.version, r.notes.as_str(), r.assets.len()),
            (Version(0, 2, 0), "Notes", 1)
        );
        assert_eq!(r.zip_name("win-x64"), "BDF-0.2.0-win-x64.zip");
        assert!(Release::parse(br#"{"tag_name":"v0.2.0","prerelease":true}"#).is_err());
        assert!(Release::parse(br#"{"tag_name":"v0.2.0","draft":true}"#).is_err());
        assert!(Release::parse(br#"{"tag_name":"nightly"}"#).is_err());
        assert!(Release::parse(b"<html>rate limited</html>").is_err());
    }

    #[test]
    fn the_real_key_verifies_an_openssl_signature() {
        // Made with: openssl pkeyutl -sign -inkey <release key> -rawin, the
        // same command the release workflow uses.
        let sig = "3a57d44a55b690d7a09e0f7e70a08d6de50f0f6c355d46e3a94fe6aebfa15c80\
                   01806f7a540e372015c61eecf19a48203c5de777bbc4814518c062b11a327d04";
        let sig: Vec<u8> = (0..64)
            .map(|i| u8::from_str_radix(&sig[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        let key = VerifyingKey::from_bytes(&public_key()).unwrap();
        let s = Signature::from_bytes(&sig.clone().try_into().unwrap());
        assert!(key.verify_strict(b"BDF update key check\n", &s).is_ok());
        assert!(key.verify_strict(b"BDF update key check!\n", &s).is_err());
    }

    #[test]
    fn a_signed_release_downloads_and_unpacks() {
        let (sk, pk) = test_key();
        let fake = fake_release(&sk);
        let mut log = Vec::new();
        assert_eq!(run(&fake, &pk, &mut log).unwrap(), program());
        // The small signed list comes first; the zip only after it checked out.
        assert_eq!(log.last().map(|s| s.ends_with(".zip")), Some(true));
        assert_eq!(log.len(), 3);
    }

    #[test]
    fn anything_not_signed_by_the_key_is_refused_before_the_big_download() {
        let (_, pk) = test_key();
        let impostor = SigningKey::from_bytes(&[9; 32]);
        let fake = fake_release(&impostor);
        let mut log = Vec::new();
        let e = run(&fake, &pk, &mut log).unwrap_err();
        assert!(e.contains("not signed"), "{e}");
        assert!(
            !log.iter().any(|n| n.ends_with(".zip")),
            "zip fetched: {log:?}"
        );
    }

    #[test]
    fn tampering_is_refused() {
        let (sk, pk) = test_key();
        let plat = platform().unwrap_or("win-x64");
        // A changed zip (signature still fine): checksum mismatch.
        let mut fake = fake_release(&sk);
        let zip = fake.release.zip_name(plat);
        fake.files.get_mut(&zip).unwrap()[100] ^= 1;
        assert!(
            run(&fake, &pk, &mut vec![])
                .unwrap_err()
                .contains("damaged")
        );
        // A changed checksum list: signature fails.
        let mut fake = fake_release(&sk);
        fake.files.get_mut(SUMS).unwrap()[0] ^= 1;
        assert!(
            run(&fake, &pk, &mut vec![])
                .unwrap_err()
                .contains("not signed")
        );
        // A truncated or garbled signature.
        let mut fake = fake_release(&sk);
        fake.files.get_mut(SIG).unwrap().pop();
        assert!(
            run(&fake, &pk, &mut vec![])
                .unwrap_err()
                .contains("malformed")
        );
        // A missing file.
        let mut fake = fake_release(&sk);
        fake.release.assets.retain(|a| a.name != SIG);
        assert!(
            run(&fake, &pk, &mut vec![])
                .unwrap_err()
                .contains("no SHA256SUMS.txt.sig")
        );
    }

    #[test]
    fn only_a_real_program_is_accepted_from_the_zip() {
        let check = |files: &[(&str, &[u8])]| {
            let zip = zip_of(files);
            unpack_program(&zip, &Sha256::digest(&zip).into())
        };
        let p = program();
        let name = program_name();
        assert!(check(&[(&format!("BDF/{name}"), &p)]).is_ok());
        assert!(
            check(&[(&format!("BDF/{name}"), b"#!/bin/sh\nrm -rf ~")])
                .unwrap_err()
                .contains("not a program")
        );
        assert!(
            check(&[("BDF/README.txt", b"x")])
                .unwrap_err()
                .contains("no ")
        );
        assert!(
            check(&[(&format!("a/{name}"), &p), (&format!("b/{name}"), &p)])
                .unwrap_err()
                .contains("more than one")
        );
        // Deeper paths and escapes out of the folder are not the program.
        assert!(check(&[(&format!("a/b/c/{name}"), &p)]).is_err());
        assert!(check(&[(&format!("../{name}"), &p)]).is_err());
        assert!(unpack_program(b"not a zip", &Sha256::digest(b"not a zip").into()).is_err());
    }

    #[test]
    fn signed_lists_in_either_sha256sum_format() {
        let (sk, pk) = test_key();
        for line in ["{h}  BDF.zip\r\n", "{h} *BDF.zip\n"] {
            let h = "ab".repeat(32);
            let sums = line.replace("{h}", &h);
            let sig = sk.sign(sums.as_bytes()).to_bytes();
            assert_eq!(
                signed_hash(sums.as_bytes(), &sig, "BDF.zip", &pk).unwrap(),
                [0xab; 32]
            );
            assert!(signed_hash(sums.as_bytes(), &sig, "BDF.zi", &pk).is_err());
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bdf-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn install_swaps_only_after_the_self_test_and_keeps_the_old_one() {
        let d = tmp("ok");
        let exe = d.join(program_name());
        std::fs::write(&exe, b"old program").unwrap();
        let mut tested = None;
        install(&exe, b"new program", |p| {
            tested = Some((p.to_path_buf(), std::fs::read(p).unwrap()));
            // The old program is untouched while the new one is tested.
            assert_eq!(std::fs::read(&exe).unwrap(), b"old program");
            Ok(())
        })
        .unwrap();
        assert_eq!(
            tested.unwrap(),
            (sibling(&exe, "new"), b"new program".to_vec())
        );
        assert_eq!(std::fs::read(&exe).unwrap(), b"new program");
        assert_eq!(std::fs::read(sibling(&exe, "old")).unwrap(), b"old program");
        assert!(!sibling(&exe, "new").exists());
        // Once the new version has started, the leftovers go.
        clean_up(&exe);
        assert!(!sibling(&exe, "old").exists());
        assert_eq!(std::fs::read(&exe).unwrap(), b"new program");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_self_test_changes_nothing() {
        let d = tmp("fail");
        let exe = d.join(program_name());
        std::fs::write(&exe, b"old program").unwrap();
        std::fs::write(sibling(&exe, "old"), b"older program").unwrap();
        let e = install(&exe, b"broken", |_| Err("crashed".into())).unwrap_err();
        assert!(
            e.contains("self-test") && e.contains("nothing was changed"),
            "{e}"
        );
        assert_eq!(std::fs::read(&exe).unwrap(), b"old program");
        assert!(!sibling(&exe, "new").exists());
        // The way back from the previous update is still there too.
        assert_eq!(
            std::fs::read(sibling(&exe, "old")).unwrap(),
            b"older program"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_that_cannot_be_written_is_reported() {
        let d = tmp("nowrite");
        let exe = d.join("missing-dir").join(program_name());
        let e = install(&exe, b"new", |_| Ok(())).unwrap_err();
        assert!(e.contains("cannot replace itself"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
