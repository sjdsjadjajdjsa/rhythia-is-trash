use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

static LAUNCH_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn write_atomic(path: &str, contents: &str) -> io::Result<()> {
    let tmp = format!("{path}.tmp");
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, Path::new(path)).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// True when files can be created inside `dir` (it is created if missing).
pub fn writable(dir: &Path) -> bool {
    let probe = dir.join(".write-test");
    fs::create_dir_all(dir).is_ok()
        && fs::write(&probe, b"").is_ok()
        && fs::remove_file(&probe).is_ok()
}

/// Where maps, settings and scores live: next to the program when that folder is writable
/// (a plain unzipped folder), otherwise a per-user folder (for example under Program Files).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn data_dir(program_dir: &Path, per_user: Option<&Path>) -> PathBuf {
    if writable(program_dir) {
        return program_dir.to_path_buf();
    }
    match per_user {
        Some(dir) if writable(dir) => dir.to_path_buf(),
        _ => program_dir.to_path_buf(),
    }
}

/// Double-click, shortcuts and AppImages can start the game in any folder, so the relative
/// maps/settings/scores paths are pinned to the data folder before anything reads them.
pub fn use_data_dir() {
    let Some(dir) = chosen_data_dir() else {
        return;
    };
    if let Ok(cwd) = std::env::current_dir() {
        let _ = LAUNCH_DIR.set(cwd);
    }
    let _ = std::env::set_current_dir(dir);
}

/// A path given on the command line, read against the folder the game was started from.
pub fn from_launch_dir(path: PathBuf) -> PathBuf {
    match LAUNCH_DIR.get() {
        Some(dir) if path.is_relative() => dir.join(path),
        _ => path,
    }
}

#[cfg(windows)]
fn chosen_data_dir() -> Option<PathBuf> {
    let program_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let per_user = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("rhythia-is-trash"));
    Some(data_dir(&program_dir, per_user.as_deref()))
}

/// An AppImage runs from a read-only mount, so it keeps its data in the XDG data folder.
/// The plain Linux folder keeps working from wherever it is started, as before.
#[cfg(not(windows))]
fn chosen_data_dir() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")?;
    let dir = appimage_data_dir(
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )?;
    writable(&dir).then_some(dir)
}

#[cfg(not(windows))]
fn appimage_data_dir(xdg_data_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = xdg_data_home
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".local/share")))?;
    Some(base.join("rhythia-is-trash"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_prefers_the_program_folder_then_the_user_folder() {
        let root = std::env::temp_dir().join(format!("rhythm-datadir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let program = root.join("program");
        let user = root.join("user");
        assert_eq!(data_dir(&program, Some(&user)), program);
        assert!(!program.join(".write-test").exists());
        let blocked = root.join("blocked");
        fs::write(&blocked, b"a file, not a folder").unwrap();
        assert_eq!(data_dir(&blocked.join("sub"), Some(&user)), user);
        assert_eq!(data_dir(&blocked.join("sub"), None), blocked.join("sub"));
        assert_eq!(
            data_dir(&blocked.join("sub"), Some(&blocked.join("x"))),
            blocked.join("sub")
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn appimage_data_follows_xdg_then_home() {
        let xdg = PathBuf::from("/xdg/data");
        let home = PathBuf::from("/home/me");
        assert_eq!(
            appimage_data_dir(Some(xdg.clone()), Some(home.clone())),
            Some(xdg.join("rhythia-is-trash"))
        );
        assert_eq!(
            appimage_data_dir(Some(PathBuf::from("relative")), Some(home.clone())),
            Some(home.join(".local/share/rhythia-is-trash"))
        );
        assert_eq!(
            appimage_data_dir(None, Some(home.clone())),
            Some(home.join(".local/share/rhythia-is-trash"))
        );
        assert_eq!(appimage_data_dir(None, None), None);
    }

    #[test]
    fn replaces_the_file_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("rhythm-store-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.json");
        let path = path.to_str().unwrap();
        write_atomic(path, "one").unwrap();
        write_atomic(path, "two").unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "two");
        assert!(!Path::new(&format!("{path}.tmp")).exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
