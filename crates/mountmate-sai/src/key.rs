use crate::Error;
use mountmate_core::storage::{FileLock, atomic_write, restrict_private_path};
use sha2::{Digest, Sha256};
use ssh_key::{HashAlg, LineEnding, PrivateKey, private::Ed25519Keypair};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;
use zeroize::Zeroizing;

pub(crate) struct LocalKey {
    pub path: PathBuf,
    pub public_key: String,
    pub fingerprint: String,
    pub request_id: String,
}
pub(crate) fn load_or_create(data_dir: &Path, context: &[u8]) -> Result<LocalKey, Error> {
    let (directory, _lock) = key_directory(data_dir, context)?;
    load_current(&directory, context)
}

/// A human-requested replacement never overwrites a key used by a saved connection.
pub(crate) fn renew(
    data_dir: &Path,
    context: &[u8],
    retired_request_id: &str,
) -> Result<LocalKey, Error> {
    let (directory, _lock) = key_directory(data_dir, context)?;
    let current = load_current(&directory, context)?;
    if current.request_id != retired_request_id {
        // Another setup attempt already replaced the retired intent. Resume it.
        return Ok(current);
    }
    let private = PrivateKey::from(Ed25519Keypair::random(&mut rand_core::OsRng));
    let next = describe(&private, context, |id| {
        directory.join(format!("id_ed25519.{id}"))
    })?;
    if !matches!(fs::symlink_metadata(&next.path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        return Err(Error::KeyStorage);
    }
    write_key(&next.path, &private)?;
    // Publish only after writing the new private file. A failed/ambiguous POST
    // or an app restart must keep using this exact new key and request identity.
    atomic_write(&directory.join("current-key"), next.request_id.as_bytes())
        .map_err(|_| Error::KeyStorage)?;
    Ok(next)
}

fn key_directory(data_dir: &Path, context: &[u8]) -> Result<(PathBuf, FileLock), Error> {
    fs::create_dir_all(data_dir).map_err(|_| Error::KeyStorage)?;
    let root = data_dir.join("sai-keys");
    private_directory(&root)?;
    // Untrusted identifiers never become literal filenames.
    let directory = root.join(format!("{:x}", Sha256::digest(context)));
    private_directory(&directory)?;
    let lock = FileLock::acquire(&directory.join("key.lock"), Duration::from_secs(5))
        .map_err(|_| Error::KeyStorage)?;
    Ok((directory, lock))
}

fn load_current(directory: &Path, context: &[u8]) -> Result<LocalKey, Error> {
    let current = current_key_id(directory)?;
    let path = directory.join(
        current
            .as_ref()
            .map_or_else(|| "id_ed25519".to_owned(), |id| format!("id_ed25519.{id}")),
    );
    let private = match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(Error::KeyStorage);
            }
            restrict_private_path(&path, false).map_err(|_| Error::KeyStorage)?;
            let mut content = Zeroizing::new(Vec::new());
            File::open(&path)
                .map_err(|_| Error::KeyStorage)?
                .take(16 * 1024)
                .read_to_end(&mut content)
                .map_err(|_| Error::KeyStorage)?;
            let key =
                PrivateKey::from_openssh(content.as_slice()).map_err(|_| Error::KeyStorage)?;
            if !key.algorithm().is_ed25519() || key.is_encrypted() {
                return Err(Error::KeyStorage);
            }
            key
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && current.is_none() => {
            let key = PrivateKey::from(Ed25519Keypair::random(&mut rand_core::OsRng));
            write_key(&path, &key)?;
            key
        }
        Err(_) => return Err(Error::KeyStorage),
    };
    let key = describe(&private, context, |_| path)?;
    if current.is_some_and(|id| id != key.request_id) {
        return Err(Error::KeyStorage);
    }
    Ok(key)
}

fn current_key_id(directory: &Path) -> Result<Option<String>, Error> {
    let path = directory.join("current-key");
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata) if metadata.file_type().is_file() => {}
        _ => return Err(Error::KeyStorage),
    }
    restrict_private_path(&path, false).map_err(|_| Error::KeyStorage)?;
    let mut id = String::new();
    File::open(path)
        .map_err(|_| Error::KeyStorage)?
        .take(65)
        .read_to_string(&mut id)
        .map_err(|_| Error::KeyStorage)?;
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::KeyStorage);
    }
    Ok(Some(id))
}

fn write_key(path: &Path, key: &PrivateKey) -> Result<(), Error> {
    let content = key
        .to_openssh(LineEnding::LF)
        .map_err(|_| Error::KeyStorage)?;
    atomic_write(path, content.as_bytes()).map_err(|_| Error::KeyStorage)
}

fn describe(
    private: &PrivateKey,
    context: &[u8],
    path: impl FnOnce(&str) -> PathBuf,
) -> Result<LocalKey, Error> {
    let public_key = private
        .public_key()
        .to_openssh()
        .map_err(|_| Error::KeyStorage)?;
    let fingerprint = private
        .public_key()
        .fingerprint(HashAlg::Sha256)
        .to_string();
    let mut digest = Sha256::new();
    digest.update(context);
    digest.update(public_key.as_bytes());
    let request_id = format!("{:x}", digest.finalize());
    Ok(LocalKey {
        path: path(&request_id),
        public_key,
        fingerprint,
        request_id,
    })
}
fn private_directory(path: &Path) -> Result<(), Error> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(Error::KeyStorage),
    }
    if !fs::symlink_metadata(path)
        .map_err(|_| Error::KeyStorage)?
        .file_type()
        .is_dir()
    {
        return Err(Error::KeyStorage);
    }
    restrict_private_path(path, true).map_err(|_| Error::KeyStorage)
}
