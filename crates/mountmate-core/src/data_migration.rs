//! Explicit, resumable Windows migration. Discovery itself never mutates disk.
use crate::paths::AppPaths;
use std::{fs, io, path::Path};

pub fn migrate_windows_data(legacy: &AppPaths, current: &AppPaths) -> Result<(), String> {
    // Validate all trees before moving anything. Never overwrite conflicting
    // data and never traverse a symlink or Windows junction.
    let mut pairs = vec![
        (legacy.config_dir.clone(), current.config_dir.clone()),
        (legacy.cache_dir.clone(), current.cache_dir.clone()),
        (legacy.state_dir.clone(), current.state_dir.clone()),
    ];
    pairs.extend(
        legacy
            .legacy_managed_bin_dirs()
            .into_iter()
            .map(|from| (from, current.managed_bin_dir())),
    );
    for (from, to) in &pairs {
        validate_ancestors(from).map_err(|e| e.to_string())?;
        validate_ancestors(to).map_err(|e| e.to_string())?;
        preflight(from, to).map_err(|e| e.to_string())?;
    }
    for (from, to) in &pairs {
        merge(from, to).map_err(|e| e.to_string())?;
    }
    if current.settings_file().exists() {
        let mut settings = crate::storage::load_settings(current).map_err(|e| e.to_string())?;
        if let Ok(relative) = settings.cache_root.strip_prefix(&legacy.cache_dir) {
            settings.cache_root = current.cache_dir.join(relative);
            crate::storage::save_settings(current, &settings).map_err(|e| e.to_string())?;
        }
    }
    if let Some(root) = legacy.cache_dir.parent() {
        let _ = fs::remove_dir(root);
    }
    for bin in legacy.legacy_managed_bin_dirs() {
        if let Some(root) = bin.parent() {
            let _ = fs::remove_dir(root);
        }
    }
    Ok(())
}

pub(crate) fn validate_ancestors(path: &Path) -> io::Result<()> {
    for ancestor in path.ancestors() {
        inspect(ancestor)?;
    }
    Ok(())
}

pub(crate) fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

fn inspect(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(m) if is_link(&m) => Err(io::Error::other(format!(
            "Refusing linked application path: {}",
            path.display()
        ))),
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn preflight(from: &Path, to: &Path) -> io::Result<()> {
    let Some(source) = inspect(from)? else {
        return Ok(());
    };
    let destination = inspect(to)?;
    if source.is_dir() {
        if destination.is_some_and(|m| !m.is_dir()) {
            return Err(io::Error::other(format!(
                "Migration conflict: {}",
                to.display()
            )));
        }
        for entry in fs::read_dir(from)? {
            let e = entry?;
            preflight(&e.path(), &to.join(e.file_name()))?;
        }
    } else if !source.is_file() || destination.is_some() {
        return Err(io::Error::other(format!(
            "Migration conflict; original retained: {}",
            from.display()
        )));
    }
    Ok(())
}
fn merge(from: &Path, to: &Path) -> io::Result<()> {
    let Some(source) = inspect(from)? else {
        return Ok(());
    };
    if source.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let e = entry?;
            merge(&e.path(), &to.join(e.file_name()))?;
        }
        fs::remove_dir(from)
    } else {
        // Recheck immediately before rename; migration is serialized by the
        // old and new instance locks at the application entry point.
        if to.exists() {
            return Err(io::Error::other("Migration destination appeared"));
        }
        fs::rename(from, to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn paths(root: &Path) -> AppPaths {
        let root = fs::canonicalize(root.parent().unwrap())
            .unwrap()
            .join(root.file_name().unwrap());
        AppPaths {
            config_dir: root.join("config"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            data_dir: root.to_owned(),
        }
    }
    #[test]
    fn migrates_settings_and_cache_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let old = paths(&temp.path().join("old"));
        let new = paths(&temp.path().join("new"));
        crate::storage::save_settings(
            &old,
            &crate::Settings {
                cache_root: old.cache_dir.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        fs::create_dir_all(old.cache_dir.join("pending")).unwrap();
        fs::write(old.cache_dir.join("pending/data"), b"upload").unwrap();
        let old_bin = old.legacy_managed_bin_dirs().remove(0);
        fs::create_dir_all(&old_bin).unwrap();
        fs::write(old_bin.join("rclone"), b"dependency").unwrap();
        migrate_windows_data(&old, &new).unwrap();
        migrate_windows_data(&old, &new).unwrap();
        assert_eq!(
            crate::storage::load_settings(&new).unwrap().cache_root,
            new.cache_dir
        );
        assert_eq!(
            fs::read(new.cache_dir.join("pending/data")).unwrap(),
            b"upload"
        );
        assert_eq!(
            fs::read(new.managed_bin_dir().join("rclone")).unwrap(),
            b"dependency"
        );
        assert!(!old_bin.exists());
    }
    #[test]
    fn conflicts_preserve_both_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let old = paths(&temp.path().join("old"));
        let new = paths(&temp.path().join("new"));
        for p in [&old, &new] {
            fs::create_dir_all(&p.config_dir).unwrap();
        }
        fs::write(old.servers_file(), b"old").unwrap();
        fs::write(new.servers_file(), b"new").unwrap();
        assert!(migrate_windows_data(&old, &new).is_err());
        assert_eq!(fs::read(old.servers_file()).unwrap(), b"old");
        assert_eq!(fs::read(new.servers_file()).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn linked_parent_cannot_redirect_migration() {
        let temp = tempfile::tempdir().unwrap();
        let old = paths(&temp.path().join("old"));
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, &old.data_dir).unwrap();
        fs::create_dir(old.config_dir.clone()).unwrap();
        fs::write(old.servers_file(), b"keep").unwrap();
        let new = paths(&temp.path().join("new"));
        assert!(migrate_windows_data(&old, &new).is_err());
        assert_eq!(fs::read(old.servers_file()).unwrap(), b"keep");
    }
}
