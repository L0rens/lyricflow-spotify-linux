use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use std::{
    ffi::OsString,
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

pub fn config_dir() -> Result<PathBuf> {
    config_directory(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

pub fn cache_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Ok(path.join("lyricflow"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .context("Neither an absolute XDG_CACHE_HOME nor HOME is available")?;
    Ok(home.join(".cache/lyricflow"))
}

fn config_directory(xdg: Option<OsString>, home: Option<OsString>) -> Result<PathBuf> {
    if let Some(path) = xdg.map(PathBuf::from).filter(|path| path.is_absolute()) {
        return Ok(path.join("lyricflow"));
    }
    let home = home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .context("Neither an absolute XDG_CONFIG_HOME nor HOME is available")?;
    Ok(home.join(".config/lyricflow"))
}

fn create_private(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

struct Temporary(PathBuf);

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .context("Configuration path must include a directory")?;
    fs::create_dir_all(parent).context("Could not create configuration directory")?;
    let (temporary, mut file) = loop {
        let candidate = parent.join(format!(".lyricflow-{:016x}.tmp", rand::random::<u64>()));
        match create_private(&candidate) {
            Ok(file) => break (Temporary(candidate), file),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).context("Could not create temporary configuration file");
            }
        }
    };
    file.write_all(data)
        .context("Could not write configuration")?;
    file.sync_all().context("Could not sync configuration")?;
    drop(file);
    fs::rename(&temporary.0, path).context("Could not replace configuration file")?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> Result<(T, Option<String>)> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok((T::default(), None)),
        Err(error) => return Err(error).context("Could not read settings"),
    };
    match serde_json::from_slice(&bytes) {
        Ok(settings) => Ok((settings, None)),
        Err(_) => {
            let parent = path.parent().context("Invalid settings path")?;
            let name = path
                .file_name()
                .context("Invalid settings filename")?
                .to_string_lossy();
            for suffix in 0..1_000 {
                let backup = parent.join(if suffix == 0 {
                    format!("{name}.bak")
                } else {
                    format!("{name}.bak.{suffix}")
                });
                let mut file = match create_private(&backup) {
                    Ok(file) => file,
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error).context("Could not back up invalid settings"),
                };
                if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
                    drop(file);
                    let _ = fs::remove_file(&backup);
                    return Err(error).context("Could not back up invalid settings");
                }
                return Ok((
                    T::default(),
                    Some(format!(
                        "Could not parse settings. Using defaults; backup saved as {}",
                        backup.display()
                    )),
                ));
            }
            bail!("Could not allocate a settings backup filename")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> TestDirectory {
        let path =
            std::env::temp_dir().join(format!("lyricflow-storage-{:016x}", rand::random::<u64>()));
        fs::create_dir(&path).unwrap();
        TestDirectory(path)
    }

    struct TestDirectory(PathBuf);
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn xdg_absolute_paths_take_precedence_and_relative_paths_fall_back() {
        assert_eq!(
            config_directory(Some("/tmp/config".into()), Some("/tmp/home".into())).unwrap(),
            PathBuf::from("/tmp/config/lyricflow")
        );
        for xdg in [None, Some("".into()), Some("relative".into())] {
            assert_eq!(
                config_directory(xdg, Some("/tmp/home".into())).unwrap(),
                PathBuf::from("/tmp/home/.config/lyricflow")
            );
        }
        assert!(config_directory(None, None).is_err());
    }

    #[test]
    fn atomic_replacement_preserves_valid_json_and_private_permissions() {
        let temp = directory();
        let path = temp.0.join("settings.json");
        write_atomic(&path, br#"{"value":1}"#).unwrap();
        write_atomic(&path, br#"{"value":2}"#).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"value":2}"#);
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(&temp.0).unwrap();
    }

    #[test]
    fn corrupt_settings_are_backed_up_without_overwriting_earlier_backups() {
        let temp = directory();
        let path = temp.0.join("settings.json");
        fs::write(&path, "broken JSON").unwrap();
        fs::write(temp.0.join("settings.json.bak"), "older backup").unwrap();
        let (_, warning) = load_json::<serde_json::Value>(&path).unwrap();
        assert!(warning.unwrap().contains("settings.json.bak.1"));
        assert_eq!(
            fs::read_to_string(temp.0.join("settings.json.bak")).unwrap(),
            "older backup"
        );
        assert_eq!(
            fs::read_to_string(temp.0.join("settings.json.bak.1")).unwrap(),
            "broken JSON"
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "broken JSON");
        fs::remove_dir_all(&temp.0).unwrap();
    }

    #[test]
    fn failed_replacement_leaves_existing_target_and_cleans_temporary_file() {
        let temp = directory();
        let path = temp.0.join("settings.json");
        fs::create_dir(&path).unwrap();
        assert!(write_atomic(&path, b"new").is_err());
        assert!(path.is_dir());
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
        fs::remove_dir_all(&temp.0).unwrap();
    }
}
