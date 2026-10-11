use anyhow::{bail, Context, Result};
use rustix::fs::{openat2, AtFlags, Mode, OFlags, ResolveFlags};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

const MAX_FILE: usize = 1024 * 1024;
#[derive(Clone)]
pub struct FileAccess {
    pub cwd: PathBuf,
    roots: Vec<(PathBuf, Arc<File>, bool)>,
}
impl FileAccess {
    pub fn new(cwd: PathBuf, read_roots: Vec<PathBuf>, write_roots: Vec<PathBuf>) -> Result<Self> {
        let mut roots = Vec::new();
        for root in read_roots {
            let writable = write_roots.contains(&root);
            let directory = if root.is_file() {
                bail!("individual-file grants are not supported by coding tools; grant the containing directory");
            } else {
                root.clone()
            };
            roots.push((
                directory.clone(),
                Arc::new(File::open(directory)?),
                writable,
            ));
        }
        roots.sort_by_key(|(path, _, _)| std::cmp::Reverse(path.components().count()));
        Ok(Self { cwd, roots })
    }
    fn resolve(&self, path: &Path, write: bool) -> Result<(&File, PathBuf)> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        };
        if absolute
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            bail!("parent traversal is not allowed");
        }
        for (root, fd, writable) in &self.roots {
            if let Ok(relative) = absolute.strip_prefix(root) {
                if write && !writable {
                    bail!("path is read-only");
                }
                if relative
                    .components()
                    .any(|part| part.as_os_str() == ".git" || part.as_os_str() == ".bare")
                {
                    bail!("direct Git metadata access is disabled");
                }
                return Ok((
                    fd,
                    if relative.as_os_str().is_empty() {
                        PathBuf::from(".")
                    } else {
                        relative.into()
                    },
                ));
            }
        }
        bail!("path is outside launcher grants")
    }
    fn open(&self, path: &Path, write: bool) -> Result<File> {
        let (root, relative) = self.resolve(path, write)?;
        Ok(openat2(
            root,
            &relative,
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )?
        .into())
    }
    pub fn read(&self, path: &Path) -> Result<String> {
        let file = self.open(path, false)?;
        Self::read_file(file)
    }
    fn read_file(file: File) -> Result<String> {
        if !file.metadata()?.is_file() {
            bail!("not a regular file");
        }
        let mut bytes = Vec::new();
        file.take((MAX_FILE + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_FILE || bytes.contains(&0) {
            bail!("file is binary or exceeds 1 MiB");
        }
        String::from_utf8(bytes).context("file is not UTF-8")
    }
    fn parent(&self, path: &Path) -> Result<(File, std::ffi::OsString)> {
        let (root, relative) = self.resolve(path, true)?;
        let name = relative
            .file_name()
            .context("missing filename")?
            .to_os_string();
        let parent = relative
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let directory: File = openat2(
            root,
            parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )?
        .into();
        Ok((directory, name))
    }
    pub fn check(&self, path: &Path, expected: Option<&str>) -> Result<()> {
        self.parent(path)?;
        Self::check_text(self.read(path), expected)
    }
    fn check_at(parent: &File, name: &std::ffi::OsStr, expected: Option<&str>) -> Result<()> {
        let text = (|| -> Result<String> {
            let file = openat2(
                parent,
                name,
                OFlags::RDONLY | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
            )?;
            Self::read_file(file.into())
        })();
        Self::check_text(text, expected)
    }
    fn check_text(text: Result<String>, expected: Option<&str>) -> Result<()> {
        match (text,expected) {
            (Ok(text),Some(expected)) if digest(&text)==expected=>Ok(()),
            (Err(error),None) if error.downcast_ref::<rustix::io::Errno>()==Some(&rustix::io::Errno::NOENT)=>Ok(()),
            _=>bail!("file conflict: re-read the path and use its current SHA-256 (null only for a new file)"),
        }
    }
    pub fn write(&self, path: &Path, content: &str, expected: Option<&str>) -> Result<()> {
        if content.len() > MAX_FILE || content.contains('\0') {
            bail!("content is binary or exceeds 1 MiB");
        }
        let (parent, name) = self.parent(path)?;
        Self::check_at(&parent, &name, expected)?;
        let temp = format!(".ax-{}.tmp", uuid::Uuid::new_v4().simple());
        let fd = rustix::fs::openat(
            &parent,
            temp.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?;
        let result = (|| -> Result<()> {
            let mut file = File::from(fd);
            file.write_all(content.as_bytes())?;
            if let Ok(existing) = openat2(
                &parent,
                &name,
                OFlags::RDONLY | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
            ) {
                file.set_permissions(File::from(existing).metadata()?.permissions())?;
            }
            file.sync_all()?;
            Self::check_at(&parent, &name, expected)?;
            if expected.is_none() {
                rustix::fs::renameat_with(
                    &parent,
                    temp.as_str(),
                    &parent,
                    &name,
                    rustix::fs::RenameFlags::NOREPLACE,
                )?;
            } else {
                rustix::fs::renameat(&parent, temp.as_str(), &parent, &name)?;
            }
            parent.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = rustix::fs::unlinkat(&parent, temp.as_str(), AtFlags::empty());
        }
        result
    }
    pub fn delete(&self, path: &Path, expected: &str) -> Result<()> {
        let (parent, name) = self.parent(path)?;
        Self::check_at(&parent, &name, Some(expected))?;
        rustix::fs::unlinkat(&parent, &name, AtFlags::empty())?;
        parent.sync_all()?;
        Ok(())
    }
    pub fn list(&self, path: &Path) -> Result<String> {
        use std::os::fd::AsRawFd;
        let directory = self.open(path, false)?;
        if !directory.metadata()?.is_dir() {
            bail!("not a directory");
        }
        let mut entries = Vec::new();
        for entry in
            std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))?.take(1001)
        {
            let entry = entry?;
            entries.push(entry.file_name().to_string_lossy().into_owned());
        }
        let truncated = entries.len() > 1000;
        entries.truncate(1000);
        entries.sort();
        Ok(serde_json::json!({"entries":entries,"truncated":truncated}).to_string())
    }
}
pub fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
