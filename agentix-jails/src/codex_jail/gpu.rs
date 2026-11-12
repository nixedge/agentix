use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use super::{bind, setenv};

// This seam is private Rust API: production cannot redirect device discovery.
trait Host {
    fn entries(&self, path: &Path) -> Result<Vec<PathBuf>>;
    fn exists(&self, path: &Path) -> bool;
    fn read(&self, path: &Path) -> Result<String>;
    fn device(&self, path: &Path) -> Result<()>;
    fn canonical(&self, path: &Path) -> Result<PathBuf>;
    fn readable_file(&self, path: &Path) -> Result<()>;
}

struct ActualHost;

impl Host for ActualHost {
    fn entries(&self, path: &Path) -> Result<Vec<PathBuf>> {
        match fs::read_dir(path) {
            Ok(entries) => entries
                .map(|entry| entry.map(|entry| entry.path()).map_err(Into::into))
                .collect(),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(err) => {
                Err(err).with_context(|| format!("reading GPU directory {}", path.display()))
            }
        }
    }

    fn exists(&self, path: &Path) -> bool {
        path.is_file() || path.is_dir()
    }

    fn read(&self, path: &Path) -> Result<String> {
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
    }

    fn device(&self, path: &Path) -> Result<()> {
        let meta = fs::symlink_metadata(path).with_context(|| {
            format!(
                "--gpu: missing device {}; check host driver setup",
                path.display()
            )
        })?;
        ensure!(
            meta.file_type().is_char_device(),
            "--gpu: {} is not a character device",
            path.display()
        );
        // Open without issuing ioctls: test the launching user's actual access,
        // including ACLs/cgroups, rather than guessing from mode bits.
        OpenOptions::new().read(true).write(true).open(path)
            .with_context(|| format!("--gpu: cannot access device {}; check host permissions, ACLs and device policy (the launcher will not change them)", path.display()))?;
        Ok(())
    }

    fn canonical(&self, path: &Path) -> Result<PathBuf> {
        path.canonicalize()
            .with_context(|| format!("resolving GPU path {}", path.display()))
    }

    fn readable_file(&self, path: &Path) -> Result<()> {
        ensure!(
            path.is_file(),
            "--gpu: driver library {} is not a regular file",
            path.display()
        );
        fs::File::open(path).with_context(|| {
            format!(
                "--gpu: cannot read driver library {}; check host file permissions",
                path.display()
            )
        })?;
        Ok(())
    }
}

#[derive(Default, Debug)]
pub(super) struct GpuAccess {
    devices: Vec<PathBuf>,
    readonly: Vec<PathBuf>,
    symlinks: Vec<(PathBuf, PathBuf)>,
    environment: HashMap<String, String>,
}

impl GpuAccess {
    pub(super) fn discover(direnv: &HashMap<String, String>) -> Result<Self> {
        let host = std::env::vars().collect();
        let selected = selected_environment(direnv, &host)?;
        // The Nix wrapper provides only the loader, never a replacement host driver.
        let loader = std::env::var_os("CODEX_JAIL_VULKAN_LIB_DIR").map(PathBuf::from);
        Self::from_host(&ActualHost, selected, loader)
    }

    fn from_host(
        host: &impl Host,
        mut environment: HashMap<String, String>,
        loader: Option<PathBuf>,
    ) -> Result<Self> {
        let mut result = Self::default();
        let mut nvidia = Vec::new();
        for path in host.entries(Path::new("/dev"))? {
            if numbered(&path, "nvidia") {
                nvidia.push(path);
            }
        }
        let render: Vec<_> = host
            .entries(Path::new("/dev/dri"))?
            .into_iter()
            .filter(|path| numbered(path, "renderD"))
            .collect();
        ensure!(!nvidia.is_empty() || !render.is_empty(),
            "--gpu: no supported GPU devices found (/dev/nvidiaN or /dev/dri/renderDN). Run codex-jail on the GPU host, outside an existing jail, and check host driver/device setup; this flag does not provide remote GPUs");

        let mut libraries = vec![PathBuf::from("/run/opengl-driver/lib")];
        if let Some(loader) = loader {
            libraries.push(loader);
        }
        if let Some(existing) = environment.get("LD_LIBRARY_PATH") {
            libraries.extend(std::env::split_paths(existing).filter(|path| path.is_absolute()));
        }
        libraries.extend(
            [
                "/usr/lib/x86_64-linux-gnu",
                "/usr/lib64",
                "/usr/lib",
                "/lib/x86_64-linux-gnu",
                "/lib64",
            ]
            .map(PathBuf::from),
        );
        dedup(&mut libraries);

        // Fail explicitly for each present backend rather than silently hiding a
        // broken CUDA installation behind a usable Vulkan loader (or vice versa).
        if !nvidia.is_empty() {
            for path in [
                PathBuf::from("/dev/nvidiactl"),
                PathBuf::from("/dev/nvidia-uvm"),
            ] {
                host.device(&path)?;
                result.devices.push(path);
            }
            for path in nvidia {
                host.device(&path)?;
                result.devices.push(path);
            }
            result.library(host, &libraries, "libcuda.so.1")?;
            if host.exists(Path::new("/proc/driver/nvidia")) {
                result.readonly.push("/proc/driver/nvidia".into());
            }
        }

        if !render.is_empty() {
            for path in render {
                host.device(&path)?;
                // Mesa uses the render node's sysfs device metadata for PCI
                // identification. Bind only this GPU's subtree and its links.
                let sys = PathBuf::from("/sys/class/drm")
                    .join(path.file_name().context("render node name")?);
                if host.exists(&sys) {
                    let target = host.canonical(&sys)?;
                    let pci = target
                        .parent()
                        .and_then(Path::parent)
                        .context("render sysfs PCI parent")?;
                    ensure!(
                        pci.starts_with("/sys/devices"),
                        "--gpu: unexpected render sysfs target {}",
                        target.display()
                    );
                    result.readonly.push(pci.to_path_buf());
                    result.symlinks.push((target.clone(), sys));
                    let dev = host.read(&target.join("dev"))?;
                    ensure!(
                        dev.trim().split(':').count() == 2
                            && dev.trim().chars().all(|c| c.is_ascii_digit() || c == ':'),
                        "--gpu: invalid render device identifier"
                    );
                    let by_dev = PathBuf::from("/sys/dev/char").join(dev.trim());
                    if host.exists(&by_dev) {
                        result.symlinks.push((target, by_dev));
                    }
                }
                result.devices.push(path);
            }
            result.library(host, &libraries, "libvulkan.so.1")?;
            let manifests = manifests(host, &environment)?;
            ensure!(!manifests.is_empty(), "--gpu: missing Vulkan ICD manifests; check /run/opengl-driver/share/vulkan/icd.d or VK_DRIVER_FILES");
            for path in &manifests {
                let value: serde_json::Value = serde_json::from_str(&host.read(path)?)
                    .with_context(|| format!("--gpu: invalid ICD manifest {}", path.display()))?;
                let library = value["ICD"]["library_path"].as_str().with_context(|| {
                    format!("--gpu: missing ICD library_path in {}", path.display())
                })?;
                let library = PathBuf::from(library);
                if library.is_absolute() {
                    result.expose_file(host, &library)?;
                } else if library.components().count() > 1 {
                    result
                        .expose_file(host, &path.parent().context("ICD parent")?.join(library))?;
                } else {
                    result.library(host, &libraries, &library.to_string_lossy())?;
                }
                result.readonly.push(path.clone());
            }
            // VK_DRIVER_FILES takes precedence in modern loaders; set both so an
            // inherited legacy override cannot defeat the validated selection.
            let manifests = std::env::join_paths(manifests)?
                .to_string_lossy()
                .into_owned();
            environment.insert("VK_DRIVER_FILES".into(), manifests.clone());
            environment.insert("VK_ICD_FILENAMES".into(), manifests);
        }
        if host.exists(Path::new("/run/opengl-driver")) {
            result.readonly.push("/run/opengl-driver".into());
        }
        // Prepend actual discovered driver/loader directories, then retain all
        // existing entries in their order (including unrelated development libs).
        let mut final_paths: Vec<PathBuf> = result
            .environment
            .remove("LD_LIBRARY_PATH")
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default();
        if let Some(existing) = environment.remove("LD_LIBRARY_PATH") {
            final_paths.extend(std::env::split_paths(&existing));
        }
        dedup(&mut final_paths);
        environment.insert(
            "LD_LIBRARY_PATH".into(),
            std::env::join_paths(final_paths)?
                .to_string_lossy()
                .into_owned(),
        );
        result.environment = environment;
        result.devices.sort();
        result.readonly.sort();
        dedup(&mut result.readonly);
        Ok(result)
    }

    fn expose_file(&mut self, host: &impl Host, path: &Path) -> Result<()> {
        ensure!(
            host.exists(path),
            "--gpu: missing driver library {}; check host driver installation",
            path.display()
        );
        host.readable_file(path)?;
        let target = host.canonical(path)?;
        for file in [path, target.as_path()] {
            if !file.starts_with("/nix") {
                // Driver libraries load siblings through $ORIGIN. Preserve that
                // directory read-only rather than copying individual binaries.
                let parent = file.parent().context("driver library parent")?;
                ensure!(safe_library_directory(parent),
                    "--gpu: refusing broad or sensitive driver directory {}; put runtime libraries in a dedicated lib directory or Nix output", parent.display());
                self.readonly.push(parent.to_path_buf());
            }
        }
        Ok(())
    }

    fn library(&mut self, host: &impl Host, dirs: &[PathBuf], name: &str) -> Result<()> {
        let Some(dir) = dirs.iter().find(|dir| host.exists(&dir.join(name))) else {
            bail!("--gpu: missing driver library {name}; check host driver paths (/run/opengl-driver/lib) and LD_LIBRARY_PATH; CUDA toolkit stubs are not runtime drivers");
        };
        self.expose_file(host, &dir.join(name))?;
        let entry = self
            .environment
            .entry("LD_LIBRARY_PATH".into())
            .or_default();
        if !entry.is_empty() {
            entry.push(':');
        }
        entry.push_str(&dir.to_string_lossy());
        Ok(())
    }

    pub(super) fn mount(&self, b: &mut Vec<OsString>) {
        for path in &self.devices {
            bind(b, "--dev-bind", path, path);
        }
        let mut readonly = self.readonly.clone();
        readonly.sort();
        // /nix is already mounted by the launcher. A directory bind also covers
        // its children: rebinding a symlinked ICD below a read-only driver tree
        // can make bubblewrap try to create a destination inside that tree.
        let mut mounted = vec![PathBuf::from("/nix")];
        for path in &readonly {
            if mounted.iter().any(|parent| path.starts_with(parent)) {
                continue;
            }
            bind(b, "--ro-bind", path, path);
            mounted.push(path.clone());
        }
        for (target, path) in &self.symlinks {
            if let Some(parent) = path.parent() {
                b.push("--dir".into());
                b.push(parent.as_os_str().into());
            }
            bind(b, "--symlink", target, path);
        }
    }

    pub(super) fn set_environment(&self, b: &mut Vec<OsString>) {
        let mut vars: Vec<_> = self.environment.iter().collect();
        vars.sort();
        for (key, value) in vars {
            setenv(b, key, value);
        }
    }
}

fn selected_environment(
    direnv: &HashMap<String, String>,
    host: &HashMap<String, String>,
) -> Result<HashMap<String, String>> {
    let mut selected = HashMap::new();
    for key in [
        "LD_LIBRARY_PATH",
        "VK_DRIVER_FILES",
        "VK_ICD_FILENAMES",
        "CUDA_VISIBLE_DEVICES",
        "CUDA_DEVICE_ORDER",
    ] {
        if let Some(value) = direnv.get(key).or_else(|| host.get(key)) {
            selected.insert(key.to_string(), value.clone());
        }
    }
    if let (Some(dev), Some(host)) = (direnv.get("LD_LIBRARY_PATH"), host.get("LD_LIBRARY_PATH")) {
        let mut paths: Vec<_> = std::env::split_paths(dev)
            .chain(std::env::split_paths(host))
            .collect();
        dedup(&mut paths);
        selected.insert(
            "LD_LIBRARY_PATH".into(),
            std::env::join_paths(paths)?.to_string_lossy().into_owned(),
        );
    }
    Ok(selected)
}

fn safe_library_directory(path: &Path) -> bool {
    use std::path::Component;
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return false;
    }
    if path.starts_with("/run") {
        return path.starts_with("/run/opengl-driver/lib");
    }
    if [
        "/dev", "/proc", "/sys", "/home", "/root", "/etc", "/tmp", "/var",
    ]
    .iter()
    .any(|root| path.starts_with(root))
    {
        return false;
    }
    path.starts_with("/usr/lib")
        || path.starts_with("/usr/lib64")
        || path.starts_with("/lib")
        || path.starts_with("/lib64")
        || path
            .file_name()
            .is_some_and(|name| name == "lib" || name == "lib64")
}

fn numbered(path: &Path, prefix: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(prefix))
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
}

fn dedup(paths: &mut Vec<PathBuf>) {
    let mut seen = std::collections::HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
}

fn manifests(host: &impl Host, environment: &HashMap<String, String>) -> Result<Vec<PathBuf>> {
    if let Some(value) = environment
        .get("VK_DRIVER_FILES")
        .or_else(|| environment.get("VK_ICD_FILENAMES"))
    {
        let paths: Vec<_> = std::env::split_paths(value).collect();
        for path in &paths {
            ensure!(
                path.is_absolute() && host.exists(path),
                "--gpu: Vulkan ICD override must name an existing absolute file: {}",
                path.display()
            );
        }
        return Ok(paths);
    }
    let mut paths = Vec::new();
    for dir in [
        "/run/opengl-driver/share/vulkan/icd.d",
        "/etc/vulkan/icd.d",
        "/usr/share/vulkan/icd.d",
    ] {
        paths.extend(
            host.entries(Path::new(dir))?
                .into_iter()
                .filter(|path| path.extension().is_some_and(|ext| ext == "json")),
        );
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::collections::HashSet;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    #[derive(Default)]
    struct FixtureHost {
        files: HashSet<PathBuf>,
        denied: HashSet<PathBuf>,
        contents: HashMap<PathBuf, String>,
        canonical: HashMap<PathBuf, PathBuf>,
    }

    impl FixtureHost {
        fn nvidia() -> Self {
            Self {
                files: [
                    "/dev/nvidia0",
                    "/dev/nvidiactl",
                    "/dev/nvidia-uvm",
                    "/dev/nvidia-modeset",
                    "/dev/nvidia-uvm-tools",
                    "/dev/nvidia-caps/nvidia-cap1",
                    "/run/opengl-driver",
                    "/run/opengl-driver/lib/libcuda.so.1",
                    "/proc/driver/nvidia",
                ]
                .map(PathBuf::from)
                .into(),
                ..Self::default()
            }
        }

        fn vulkan() -> Self {
            Self {
                files: [
                    "/dev/dri/renderD128",
                    "/dev/dri/card0",
                    "/dev/snd/controlC0",
                    "/run/opengl-driver",
                    "/run/opengl-driver/lib/libvulkan.so.1",
                    "/run/opengl-driver/lib/libexample.so",
                    "/run/opengl-driver/share/vulkan/icd.d/example.json",
                ]
                .map(PathBuf::from)
                .into(),
                contents: [(
                    PathBuf::from("/run/opengl-driver/share/vulkan/icd.d/example.json"),
                    r#"{"ICD":{"library_path":"libexample.so"}}"#.into(),
                )]
                .into(),
                ..Self::default()
            }
        }
    }

    impl Host for FixtureHost {
        fn entries(&self, path: &Path) -> Result<Vec<PathBuf>> {
            Ok(self
                .files
                .iter()
                .filter(|file| file.parent() == Some(path))
                .cloned()
                .collect())
        }
        fn exists(&self, path: &Path) -> bool {
            self.files.contains(path)
        }
        fn read(&self, path: &Path) -> Result<String> {
            self.contents
                .get(path)
                .cloned()
                .context("missing fixture content")
        }
        fn device(&self, path: &Path) -> Result<()> {
            ensure!(
                self.files.contains(path),
                "--gpu: missing device {}",
                path.display()
            );
            ensure!(
                !self.denied.contains(path),
                "--gpu: cannot access device {}; check host permissions",
                path.display()
            );
            Ok(())
        }
        fn canonical(&self, path: &Path) -> Result<PathBuf> {
            Ok(self
                .canonical
                .get(path)
                .cloned()
                .unwrap_or_else(|| path.into()))
        }
        fn readable_file(&self, path: &Path) -> Result<()> {
            ensure!(
                !self.denied.contains(path),
                "--gpu: cannot read driver library {}; check host file permissions",
                path.display()
            );
            Ok(())
        }
    }

    #[test]
    fn environment_selection_merges_libraries_without_host_secrets() -> Result<()> {
        let host = [
            ("LD_LIBRARY_PATH".into(), "/host/lib:/shared/lib".into()),
            ("CUDA_VISIBLE_DEVICES".into(), "0".into()),
            ("HOST_SECRET".into(), "secret".into()),
        ]
        .into();
        let direnv = [("LD_LIBRARY_PATH".into(), "/devshell/lib:/shared/lib".into())].into();
        let selected = selected_environment(&direnv, &host)?;
        assert_eq!(
            selected["LD_LIBRARY_PATH"],
            "/devshell/lib:/shared/lib:/host/lib"
        );
        assert_eq!(selected["CUDA_VISIBLE_DEVICES"], "0");
        assert!(!selected.contains_key("HOST_SECRET"));
        Ok(())
    }

    #[test]
    fn library_paths_cannot_expand_to_broad_or_sensitive_mounts() -> Result<()> {
        for directory in [
            "/",
            "/run",
            "/dev",
            "/home/sam",
            "/home/sam/.ssh",
            "/run/other/lib",
            "/opt/driver/lib/../..",
        ] {
            assert!(!safe_library_directory(Path::new(directory)), "{directory}");
        }
        let mut host = FixtureHost::nvidia();
        host.canonical.insert(
            "/run/opengl-driver/lib/libcuda.so.1".into(),
            "/run/libcuda.so.1".into(),
        );
        let error = GpuAccess::from_host(&host, HashMap::new(), None).err();
        assert!(error.is_some_and(|error| error
            .to_string()
            .contains("refusing broad or sensitive driver directory /run")));
        Ok(())
    }

    #[test]
    fn nvidia_compute_mounts_exclude_management_and_preserve_libraries() -> Result<()> {
        let env = [(
            "LD_LIBRARY_PATH".into(),
            "/some/toolkit/lib:/other/lib".into(),
        )]
        .into();
        let access = GpuAccess::from_host(&FixtureHost::nvidia(), env, None)?;
        assert_eq!(
            access.devices,
            ["/dev/nvidia-uvm", "/dev/nvidia0", "/dev/nvidiactl"].map(PathBuf::from)
        );
        assert!(access.readonly.contains(&"/run/opengl-driver".into()));
        assert!(access.readonly.contains(&"/proc/driver/nvidia".into()));
        assert_eq!(
            access.environment["LD_LIBRARY_PATH"],
            "/run/opengl-driver/lib:/some/toolkit/lib:/other/lib"
        );
        Ok(())
    }

    #[test]
    fn vulkan_uses_render_nodes_and_validated_icds() -> Result<()> {
        let access = GpuAccess::from_host(&FixtureHost::vulkan(), HashMap::new(), None)?;
        assert_eq!(access.devices, [PathBuf::from("/dev/dri/renderD128")]);
        assert_eq!(
            access.environment["VK_DRIVER_FILES"],
            "/run/opengl-driver/share/vulkan/icd.d/example.json"
        );
        assert_eq!(
            access.environment["VK_DRIVER_FILES"],
            access.environment["VK_ICD_FILENAMES"]
        );
        assert!(access.readonly.contains(&"/run/opengl-driver/lib".into()));
        Ok(())
    }

    #[test]
    fn missing_devices_permissions_and_libraries_are_distinct() {
        let error = GpuAccess::from_host(&FixtureHost::default(), HashMap::new(), None).err();
        assert!(error.is_some_and(|e| e.to_string().contains("no supported GPU devices")));
        let mut host = FixtureHost::nvidia();
        host.denied.insert("/dev/nvidia0".into());
        let error = GpuAccess::from_host(&host, HashMap::new(), None).err();
        assert!(error.is_some_and(|e| e.to_string().contains("permissions")));
        host.denied.clear();
        host.denied
            .insert("/run/opengl-driver/lib/libcuda.so.1".into());
        let error = GpuAccess::from_host(&host, HashMap::new(), None).err();
        assert!(error.is_some_and(|e| e.to_string().contains("cannot read driver library")));
        host.denied.clear();
        host.files.remove(Path::new("/dev/nvidia-uvm"));
        let error = GpuAccess::from_host(&host, HashMap::new(), None).err();
        assert!(error.is_some_and(|e| e.to_string().contains("missing device /dev/nvidia-uvm")));
        host.files.insert("/dev/nvidia-uvm".into());
        host.files
            .remove(Path::new("/run/opengl-driver/lib/libcuda.so.1"));
        let error = GpuAccess::from_host(&host, HashMap::new(), None).err();
        assert!(error.is_some_and(|e| e
            .to_string()
            .contains("missing driver library libcuda.so.1")));
    }

    #[test]
    fn invalid_icd_override_fails() {
        let env = [("VK_DRIVER_FILES".into(), "relative.json".into())].into();
        assert!(GpuAccess::from_host(&FixtureHost::vulkan(), env, None).is_err());
    }

    #[test]
    fn render_sysfs_links_point_to_readonly_gpu_subtree() -> Result<()> {
        let mut host = FixtureHost::vulkan();
        let class = PathBuf::from("/sys/class/drm/renderD128");
        let target = PathBuf::from("/sys/devices/pci0000:00/0000:00:01.0/drm/renderD128");
        host.files.insert(class.clone());
        host.files.insert("/sys/dev/char/226:128".into());
        host.canonical.insert(class.clone(), target.clone());
        host.contents.insert(target.join("dev"), "226:128\n".into());
        let access = GpuAccess::from_host(&host, HashMap::new(), None)?;
        assert!(access
            .readonly
            .contains(&"/sys/devices/pci0000:00/0000:00:01.0".into()));
        assert!(access.symlinks.contains(&(target.clone(), class)));
        assert!(access
            .symlinks
            .contains(&(target, "/sys/dev/char/226:128".into())));
        Ok(())
    }

    #[test]
    fn nix_loader_and_external_driver_symlinks_are_preserved() -> Result<()> {
        let mut host = FixtureHost::vulkan();
        host.files
            .remove(Path::new("/run/opengl-driver/lib/libvulkan.so.1"));
        host.files
            .insert("/nix/store/fixture-vulkan/lib/libvulkan.so.1".into());
        host.canonical.insert(
            "/run/opengl-driver/lib/libexample.so".into(),
            "/opt/gpu-driver/lib/libexample.so".into(),
        );
        let access = GpuAccess::from_host(
            &host,
            HashMap::new(),
            Some("/nix/store/fixture-vulkan/lib".into()),
        )?;
        assert!(access.readonly.contains(&"/opt/gpu-driver/lib".into()));
        assert!(!access
            .readonly
            .iter()
            .any(|path| path.starts_with("/nix/store")));
        assert!(access.environment["LD_LIBRARY_PATH"].contains("/nix/store/fixture-vulkan/lib"));
        Ok(())
    }

    #[test]
    fn driver_tree_children_are_not_rebound() -> Result<()> {
        let access = GpuAccess::from_host(&FixtureHost::vulkan(), HashMap::new(), None)?;
        let mut args = Vec::new();
        access.mount(&mut args);
        assert_eq!(
            args,
            [
                "--dev-bind",
                "/dev/dri/renderD128",
                "/dev/dri/renderD128",
                "--ro-bind",
                "/run/opengl-driver",
                "/run/opengl-driver"
            ]
            .map(OsString::from)
        );
        Ok(())
    }

    #[test]
    fn real_bwrap_reads_symlinked_icd_without_nested_bind() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let driver = temp.path().join("driver");
        let targets = temp.path().join("targets");
        let manifest = driver.join("share/vulkan/icd.d/asahi_icd.x86_64.json");
        fs::create_dir_all(manifest.parent().context("ICD directory")?)?;
        fs::create_dir(&targets)?;
        let content = r#"{"ICD":{"library_path":"fixture-only"}}"#;
        fs::write(targets.join("asahi.json"), content)?;
        std::os::unix::fs::symlink(targets.join("asahi.json"), &manifest)?;
        let cat = std::env::split_paths(&std::env::var_os("PATH").context("PATH")?)
            .map(|dir| dir.join("cat"))
            .find(|path| path.is_file())
            .context("cat in Nix development shell")?
            .canonicalize()?;
        let access = GpuAccess {
            readonly: vec![manifest.clone(), driver],
            ..GpuAccess::default()
        };
        let mut args = Vec::new();
        super::super::push(
            &mut args,
            &["--unshare-all", "--share-net", "--ro-bind", "/nix", "/nix"],
        );
        bind(&mut args, "--ro-bind", &targets, &targets);
        access.mount(&mut args);
        args.push("--".into());
        let multicall = cat.file_name().is_some_and(|name| name == "coreutils");
        args.push(cat.into_os_string());
        if multicall {
            args.push("--coreutils-prog=cat".into());
        }
        args.push(manifest.into_os_string());
        let output = Command::new("bwrap")
            .args(args)
            .output()
            .context("running real bubblewrap regression through the Nix development shell")?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout)?, content);
        Ok(())
    }

    #[test]
    fn actual_host_rejects_regular_files_as_devices() -> Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        let error = ActualHost.device(file.path()).err();
        assert!(error.is_some_and(|e| e.to_string().contains("not a character device")));
        Ok(())
    }

    // Run the launcher in an isolated test process so HOME/current_dir changes
    // cannot race other tests. Only this test binary can inject a FixtureHost.
    #[test]
    fn launcher_gpu_argument_assembly() -> Result<()> {
        for scenario in ["default", "nvidia", "vulkan", "missing"] {
            let temp = tempfile::tempdir()?;
            let root = temp.path();
            let bwrap = root.join("bwrap");
            fs::write(&bwrap, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CAPTURE\"\n")?;
            fs::set_permissions(&bwrap, fs::Permissions::from_mode(0o755))?;
            let direnv = root.join("direnv");
            fs::write(&direnv, "#!/bin/sh\nprintf '%s' '{\"LD_LIBRARY_PATH\":\"/toolkit/lib:/unrelated\",\"VK_DRIVER_FILES\":\"/run/opengl-driver/share/vulkan/icd.d/example.json\"}'\n")?;
            fs::set_permissions(&direnv, fs::Permissions::from_mode(0o755))?;
            fs::write(root.join(".envrc"), "fixture")?;
            let status = Command::new("git")
                .args(["init", "--bare"])
                .arg(root.join(".bare"))
                .status()?;
            assert!(status.success());
            let status = Command::new("git")
                .args(["--git-dir"])
                .arg(root.join(".bare"))
                .args(["worktree", "add", "--orphan", "-b", "fixture"])
                .arg(root.join("worktree"))
                .status()?;
            assert!(status.success());
            fs::write(root.join("worktree/.envrc"), "fixture")?;
            let output = Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "gpu::tests::launcher_fixture_child",
                    "--ignored",
                    "--nocapture",
                ])
                .current_dir(root.join("worktree"))
                .env("HOME", root)
                .env("GPU_TEST_SCENARIO", scenario)
                .env(
                    "PATH",
                    format!("{}:{}", root.display(), std::env::var("PATH")?),
                )
                .env("CAPTURE", root.join("capture"))
                .env("CODEX_JAIL_BIN_DIR", root)
                .env("CODEX_JAIL_BWRAP", &bwrap)
                .output()?;
            assert!(
                output.status.success(),
                "{scenario}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if scenario == "missing" {
                assert!(!root.join("capture").exists());
                continue;
            }
            let captured = fs::read_to_string(root.join("capture"))?;
            let args: Vec<_> = captured.lines().collect();
            assert!(args.windows(3).any(|a| a
                == [
                    "--bind",
                    root.to_str().unwrap_or(""),
                    root.to_str().unwrap_or("")
                ]));
            assert!(args
                .windows(3)
                .any(|a| a == ["--ro-bind", "/fixture model", "/fixture model"]));
            if scenario == "default" {
                assert!(!args.contains(&"--dev-bind"));
            } else {
                let device = if scenario == "nvidia" {
                    "/dev/nvidia0"
                } else {
                    "/dev/dri/renderD128"
                };
                assert!(args.windows(3).any(|a| a == ["--dev-bind", device, device]));
                assert!(args
                    .windows(3)
                    .any(|a| a == ["--ro-bind", "/run/opengl-driver", "/run/opengl-driver"]));
                let values: Vec<_> = args
                    .windows(3)
                    .enumerate()
                    .filter(|(_, a)| a[0] == "--setenv" && a[1] == "LD_LIBRARY_PATH")
                    .collect();
                let (index, value) = values.last().context("final library setting")?;
                assert_eq!(value[2], "/run/opengl-driver/lib:/toolkit/lib:/unrelated");
                assert!(
                    *index
                        > args
                            .iter()
                            .position(|arg| *arg == "--clearenv")
                            .context("clearenv")?
                );
                assert!(args.iter().all(|arg| *arg != "SSH_AUTH_SOCK"));
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "subprocess fixture for launcher_gpu_argument_assembly; no hardware acceptance"]
    fn launcher_fixture_child() -> Result<()> {
        let scenario = std::env::var("GPU_TEST_SCENARIO")?;
        let mut argv = vec![
            "codex-jail",
            "--no-github-auth",
            "--all-worktrees",
            "--ro",
            "/fixture model",
        ];
        if scenario != "default" {
            argv.push("--gpu");
        }
        let args = super::super::Args::parse_from(argv);
        let outcome = super::super::run(args, |direnv| {
            assert_ne!(scenario, "default", "default launch must not discover GPUs");
            let host = match scenario.as_str() {
                "nvidia" => FixtureHost::nvidia(),
                "vulkan" => FixtureHost::vulkan(),
                _ => FixtureHost::default(),
            };
            GpuAccess::from_host(&host, direnv.clone(), None)
        });
        if scenario == "missing" {
            assert!(outcome
                .err()
                .is_some_and(|error| error.to_string().contains("no supported GPU devices")));
        } else {
            assert_eq!(outcome?, 0);
        }
        Ok(())
    }
}
