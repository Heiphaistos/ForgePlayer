//! Mise à jour automatique.
//!
//! Canal : les assets de la dernière release GitHub (dépôt public).
//!   `latest.json`      manifeste `{version, notes, url, size, sha256}`
//!   `latest.json.sig`  signature Ed25519 (base64) des octets exacts du manifeste
//!
//! La clé publique est embarquée ; la clé privée ne quitte pas le PC qui publie
//! (`outils/publier.py`). Le Setup téléchargé est jeté si sa taille ou son SHA-256
//! diffèrent du manifeste signé, et toute URL hors du dépôt est refusée.
//! Seule la copie installée par Inno Setup s'installe seule ; le zip portable est
//! seulement prévenu.

use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const PUBLIC_KEY: &str = "YohuT+OR/E3yFUeVNc0uLpCD/psRIYx5ZT6uPmCOIrU=";
const MANIFEST_URL: &str = "https://github.com/Heiphaistos/ForgePlayer/releases/latest/download/latest.json";
const URL_PREFIX: &str = "https://github.com/Heiphaistos/ForgePlayer/releases/download/";
pub const RELEASES_PAGE: &str = "https://github.com/Heiphaistos/ForgePlayer/releases/latest";
const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_SETUP: u64 = 1024 * 1024 * 1024;
pub const FIRST_CHECK_SECS: f64 = 30.0;
pub const CHECK_EVERY_SECS: f64 = 6.0 * 3600.0;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// État partagé entre les fils réseau et l'interface.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Manifest),
    Downloading { done: u64, total: u64 },
    Ready { manifest: Manifest, setup: PathBuf },
    Failed(String),
}

pub type Shared = Arc<Mutex<Status>>;

/// `v1.10.1` ou `1.10.1` -> (1, 10, 1).
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    let v = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(v)
}

fn b64(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD.decode(s.trim()).map_err(|_| "base64 invalide".to_string())
}

/// Vérifie la signature du manifeste puis le décode et contrôle son URL.
pub fn verify_manifest(manifest: &[u8], sig_b64: &str, public_key_b64: &str) -> Result<Manifest, String> {
    let bad = || "signature du manifeste invalide".to_string();
    let key: [u8; 32] = b64(public_key_b64)?.try_into().map_err(|_| bad())?;
    let sig: [u8; 64] = b64(sig_b64)?.try_into().map_err(|_| bad())?;
    VerifyingKey::from_bytes(&key)
        .map_err(|_| bad())?
        .verify(manifest, &Signature::from_bytes(&sig))
        .map_err(|_| bad())?;
    let m: Manifest = serde_json::from_slice(manifest).map_err(|e| format!("manifeste illisible : {e}"))?;
    if !url_allowed(&m.url) {
        return Err("adresse de téléchargement hors du canal officiel".into());
    }
    if parse_version(&m.version).is_none() || m.size == 0 || m.size > MAX_SETUP || m.sha256.len() != 64 {
        return Err("manifeste incohérent".into());
    }
    Ok(m)
}

pub fn url_allowed(url: &str) -> bool {
    url.strip_prefix(URL_PREFIX)
        .is_some_and(|rest| url.ends_with(".exe") && !rest.contains("..") && !rest.contains(['@', '?', '#', '\\']))
}

/// Contrôle taille et SHA-256 (hex) du fichier reçu contre le manifeste signé.
fn verify_setup(size: u64, digest_hex: &str, m: &Manifest) -> Result<(), String> {
    if size != m.size || digest_hex != m.sha256.to_ascii_lowercase() {
        return Err("fichier téléchargé corrompu (taille ou empreinte différente) : installation refusée".into());
    }
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("ForgePlayer/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}

fn get_limited(c: &reqwest::blocking::Client, url: &str, max: u64) -> Result<Vec<u8>, String> {
    let r = c.get(url).send().map_err(|e| format!("réseau : {e}"))?;
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status().as_u16()));
    }
    let mut buf = Vec::new();
    r.take(max + 1).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() as u64 > max {
        return Err("réponse trop volumineuse".into());
    }
    Ok(buf)
}

/// Mise à jour disponible (plus récente que `current`), ou `None`.
pub fn check(current: &str) -> Result<Option<Manifest>, String> {
    let c = client()?;
    let manifest = get_limited(&c, MANIFEST_URL, MAX_MANIFEST)?;
    let sig = String::from_utf8_lossy(&get_limited(&c, &format!("{MANIFEST_URL}.sig"), 1024)?).into_owned();
    let m = verify_manifest(&manifest, &sig, PUBLIC_KEY)?;
    let (latest, cur) = (parse_version(&m.version), parse_version(current));
    Ok((latest > cur).then_some(m))
}

fn setup_dir() -> PathBuf {
    std::env::temp_dir().join("ForgePlayer-update")
}

/// Télécharge le Setup, vérifie taille et empreinte (fichier supprimé sinon).
pub fn download(m: &Manifest, progress: impl Fn(u64, u64)) -> Result<PathBuf, String> {
    if !url_allowed(&m.url) {
        return Err("adresse de téléchargement hors du canal officiel".into());
    }
    let dir = setup_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(format!("ForgePlayer_v{}_Setup.exe", m.version));
    let c = reqwest::blocking::Client::builder()
        .user_agent(concat!("ForgePlayer/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(1800))
        .build()
        .map_err(|e| e.to_string())?;
    let mut r = c.get(&m.url).send().map_err(|e| format!("réseau : {e}"))?;
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status().as_u16()));
    }
    let result = (|| {
        let mut f = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
        let (mut hash, mut done, mut buf) = (Sha256::new(), 0u64, vec![0u8; 64 * 1024]);
        loop {
            let n = r.read(&mut buf).map_err(|e| format!("réseau : {e}"))?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > m.size {
                return Err("fichier plus gros qu'annoncé".to_string());
            }
            hash.update(&buf[..n]);
            f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            progress(done, m.size);
        }
        verify_setup(done, &hex(&hash.finalize()), m)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&dest);
        return Err(e);
    }
    Ok(dest)
}

/// Lance une vérification sur un fil de fond (sans effet si une opération est déjà en cours).
pub fn spawn_check(shared: &Shared) {
    {
        let mut s = shared.lock().unwrap();
        if matches!(*s, Status::Checking | Status::Downloading { .. } | Status::Ready { .. }) {
            return;
        }
        *s = Status::Checking;
    }
    let shared = shared.clone();
    std::thread::spawn(move || {
        let r = check(env!("CARGO_PKG_VERSION"));
        *shared.lock().unwrap() = match r {
            Ok(Some(m)) => Status::Available(m),
            Ok(None) => Status::UpToDate,
            Err(e) => {
                log::warn!("mise à jour : {e}");
                Status::Failed(e)
            }
        };
    });
}

pub fn spawn_download(shared: &Shared, m: Manifest) {
    *shared.lock().unwrap() = Status::Downloading { done: 0, total: m.size };
    let shared = shared.clone();
    std::thread::spawn(move || {
        let s2 = shared.clone();
        let r = download(&m, |done, total| *s2.lock().unwrap() = Status::Downloading { done, total });
        *shared.lock().unwrap() = match r {
            Ok(setup) => Status::Ready { manifest: m, setup },
            Err(e) => Status::Failed(e),
        };
    });
}

/// Vrai si l'exécutable a été posé par Inno Setup (le désinstalleur est à côté).
pub fn is_installed_copy() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("unins000.exe").exists()))
        .unwrap_or(false)
}

/// Contenu du script détaché : attend la fermeture, arrête les deux services annexes (ils verrouillent
/// leurs .exe), lance le Setup silencieux (qui relance l'appli via /RELANCER=1).
fn apply_script(setup: &Path, log: &Path) -> String {
    [
        "@echo off".to_string(),
        "ping -n 3 127.0.0.1 >nul".to_string(),
        "taskkill /F /IM subtitle-service.exe >nul 2>&1".to_string(),
        "taskkill /F /IM media-indexer.exe >nul 2>&1".to_string(),
        format!("\"{}\" /SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /RELANCER=1", setup.display()),
        format!("echo %date% %time% setup code %errorlevel% >> \"{}\"", log.display()),
    ]
    .join("\r\n")
        + "\r\n"
}

/// À appeler pendant la fermeture de l'appli : le script détaché survit au processus.
#[cfg(windows)]
pub fn launch_installer(setup: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let dir = setup.parent().unwrap_or(Path::new("."));
    let bat = dir.join("apply.cmd");
    if std::fs::write(&bat, apply_script(setup, &dir.join("update.log"))).is_err() {
        return;
    }
    let _ = std::process::Command::new("cmd")
        .raw_arg(format!("/C \"\"{}\"\"", bat.display()))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

#[cfg(not(windows))]
pub fn launch_installer(_setup: &Path) {}

/// Ouvre la page des releases dans le navigateur (copie portable).
pub fn open_releases_page() {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd").args(["/C", "start", "", RELEASES_PAGE]).spawn();
    }
}

/// Supprime les Setup des mises à jour précédentes (~90 Mo chacun).
pub fn cleanup_old() {
    let _ = std::fs::remove_dir_all(setup_dir());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const SEED: [u8; 32] = [7; 32];

    fn pubkey() -> String {
        base64::engine::general_purpose::STANDARD.encode(SigningKey::from_bytes(&SEED).verifying_key().to_bytes())
    }

    fn sign(data: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(SigningKey::from_bytes(&SEED).sign(data).to_bytes())
    }

    fn manifest_json(url: &str) -> Vec<u8> {
        format!(
            r#"{{"version":"1.11.0","notes":"n","url":"{url}","size":3,"sha256":"{}"}}"#,
            hex(&Sha256::digest(b"abc"))
        )
        .into_bytes()
    }

    const GOOD_URL: &str = "https://github.com/Heiphaistos/ForgePlayer/releases/download/v1.11.0/ForgePlayer_v1.11.0_Setup.exe";

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("v1.10.0") > parse_version("1.9.9"));
        assert!(parse_version("1.11.0") > parse_version("1.10.1"));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("nightly"), None);
    }

    #[test]
    fn valid_signature_is_accepted() {
        let m = manifest_json(GOOD_URL);
        let parsed = verify_manifest(&m, &sign(&m), &pubkey()).unwrap();
        assert_eq!(parsed.version, "1.11.0");
        assert_eq!(parsed.size, 3);
    }

    #[test]
    fn tampered_manifest_or_wrong_key_is_rejected() {
        let m = manifest_json(GOOD_URL);
        let sig = sign(&m);
        let mut tampered = m.clone();
        tampered[12] = b'9';
        assert!(verify_manifest(&tampered, &sig, &pubkey()).is_err());
        let other = base64::engine::general_purpose::STANDARD.encode(SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes());
        assert!(verify_manifest(&m, &sig, &other).is_err());
        assert!(verify_manifest(&m, "pas-du-base64", &pubkey()).is_err());
        // la clé réellement embarquée est bien formée
        assert!(b64(PUBLIC_KEY).unwrap().len() == 32);
    }

    #[test]
    fn url_outside_channel_is_refused() {
        for bad in [
            "http://github.com/Heiphaistos/ForgePlayer/releases/download/v1/x.exe",
            "https://evil.example/Heiphaistos/ForgePlayer/releases/download/v1/x.exe",
            "https://github.com/Heiphaistos/ForgePlayer/releases/download/../../x.exe",
            "https://github.com/Heiphaistos/ForgePlayer/releases/download/v1/x.msi",
            "https://github.com/Heiphaistos/ForgePlayer/releases/download/@evil.com/x.exe",
        ] {
            assert!(!url_allowed(bad), "{bad}");
            let m = manifest_json(bad);
            assert!(verify_manifest(&m, &sign(&m), &pubkey()).is_err(), "{bad}");
        }
        assert!(url_allowed(GOOD_URL));
    }

    #[test]
    fn different_hash_or_size_is_refused() {
        let m = verify_manifest(&manifest_json(GOOD_URL), &sign(&manifest_json(GOOD_URL)), &pubkey()).unwrap();
        let h = |d: &[u8]| hex(&Sha256::digest(d));
        assert!(verify_setup(3, &h(b"abc"), &m).is_ok());
        assert!(verify_setup(3, &h(b"abd"), &m).is_err());
        assert!(verify_setup(4, &h(b"abc"), &m).is_err());
    }

    #[test]
    fn apply_script_quotes_paths_and_kills_sidecars() {
        let s = apply_script(Path::new(r"C:\a b\Setup.exe"), Path::new(r"C:\a b\u.log"));
        assert!(s.contains("\"C:\\a b\\Setup.exe\" /SILENT"));
        assert!(s.contains("/RELANCER=1"));
        assert!(s.contains("taskkill /F /IM media-indexer.exe"));
        assert!(s.ends_with("\r\n"));
    }
}
