//! The folders an isolated run's database files may lie in ([`crate::Target::allowed_dirs`]): the
//! one check a file driver runs inside its open, before it opens anything.
//!
//! The file and the folders are resolved first: links followed, `.` and `..` gone. A file that is
//! not there yet (not even as a link) is resolved by its folder. A path that cannot be resolved is
//! refused.

use std::path::{Path, PathBuf};

use crate::DbError;

/// The path to open for `file` when it lies in one of the `allowed` folders, both resolved; else
/// [`DbError::NotAllowed`] naming `file` as given.
pub fn allowed_file(allowed: &[PathBuf], file: &Path) -> Result<PathBuf, DbError> {
    if let Some(path) = resolve(file)
        && allowed.iter().filter_map(|dir| std::fs::canonicalize(dir).ok()).any(|dir| path.starts_with(dir))
    {
        return Ok(path);
    }
    Err(DbError::NotAllowed(format!("this isolated run may only open files in its own folders; {} is not in them", file.display())))
}

/// `path` with its links followed and `.` and `..` gone; a path that is not there, by its folder.
fn resolve(path: &Path) -> Option<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => std::fs::canonicalize(path).ok(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let name = path.file_name()?;
            let folder = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            Some(std::fs::canonicalize(folder).ok()?.join(name))
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own under `target/`, removed when dropped.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Folder {
            let path = dbee_testkit::repo_root().join("target").join("testkit").join("allowed").join(dbee_testkit::unique(name));
            std::fs::create_dir_all(&path).unwrap();
            Folder(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn files_must_lie_in_the_allowed_folders_once_resolved() {
        let (allowed, outside) = (Folder::new("in"), Folder::new("out"));
        let inside = allowed.0.join("in.sqlite");
        let out = outside.0.join("out.sqlite");
        std::fs::write(&inside, "").unwrap();
        std::fs::write(&out, "").unwrap();
        let dirs = vec![allowed.0.clone()];
        let check = |path: &Path| allowed_file(&dirs, path);
        let resolved = std::fs::canonicalize(&inside).unwrap();
        assert_eq!(check(&inside), Ok(resolved.clone()));
        // A file not there yet is resolved by its folder; `..` that stays inside is fine.
        assert_eq!(check(&allowed.0.join("new.sqlite")), Ok(resolved.with_file_name("new.sqlite")));
        let round = allowed.0.join("..").join(allowed.0.file_name().unwrap()).join("in.sqlite");
        assert_eq!(check(&round), Ok(resolved.clone()));
        let back_out = allowed.0.join("..").join(outside.0.file_name().unwrap()).join("out.sqlite");
        let mut refused =
            vec![out.clone(), outside.0.join("new.sqlite"), PathBuf::from("/etc/hosts"), back_out, allowed.0.join("gone").join("x.sqlite")];
        #[cfg(unix)]
        {
            // A link out of the folder is refused, whether or not what it names is there; a link to
            // a file inside opens that file, and a folder may be given through a link.
            use std::os::unix::fs::symlink;
            symlink("/etc/hosts", allowed.0.join("hosts.sqlite")).unwrap();
            symlink(outside.0.join("missing.sqlite"), allowed.0.join("dangling.sqlite")).unwrap();
            symlink(&inside, allowed.0.join("alias.sqlite")).unwrap();
            refused.extend([allowed.0.join("hosts.sqlite"), allowed.0.join("dangling.sqlite")]);
            assert_eq!(check(&allowed.0.join("alias.sqlite")), Ok(resolved.clone()));
            symlink(&allowed.0, outside.0.join("to-allowed")).unwrap();
            assert_eq!(allowed_file(&[outside.0.join("to-allowed")], &inside), Ok(resolved));
        }
        for path in &refused {
            let e = check(path).unwrap_err();
            assert_eq!(e.code(), "config", "{}: {e}", path.display());
            assert_eq!(e.to_string(), format!("this isolated run may only open files in its own folders; {} is not in them", path.display()));
        }
        // An empty list allows nothing.
        assert!(allowed_file(&[], &inside).is_err());
    }
}
