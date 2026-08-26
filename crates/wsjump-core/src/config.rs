//! Configuration and the install directory, including the shared root.
//!
//! `wsjump.toml` lives in the *install* directory, which is also the default
//! home of `wsjump.db`. What "install directory" means differs per platform
//! because `current_exe()` answers differently: see [`install_dir`].

use std::fs;
#[cfg(unix)]
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::query::SortOrder;

/// Name of the configuration file, looked up in the install directory.
const CONFIG_FILE_NAME: &str = "wsjump.toml";

/// Parsed v3 config. Unknown keys and relative roots are rejected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Absolute root containing the flat `w/` and categorized `c/` containers.
    pub root_dir: PathBuf,
    /// Optional database location; defaults beside the invoked executable.
    #[serde(default)]
    db_file: Option<PathBuf>,
    /// Optional query order; defaults to recency.
    #[serde(default)]
    default_sort: Option<SortOrder>,
}

impl Config {
    /// Load `wsjump.toml` from the install directory and validate its root.
    pub fn load() -> AppResult<Self> {
        let path = install_dir()?.join(CONFIG_FILE_NAME);
        let text = fs::read_to_string(&path).map_err(|error| {
            AppError::runtime(format!(
                "cannot read config file '{}': {}",
                path.display(),
                error
            ))
        })?;
        Self::parse(&text)
    }

    /// Parse v3 settings; legacy keys are deliberately unsupported.
    fn parse(text: &str) -> AppResult<Self> {
        let config: Self = toml::from_str(text)
            .map_err(|error| AppError::runtime(format!("cannot parse config: {}", error)))?;
        if !config.root_dir.is_absolute() {
            return Err(AppError::runtime("root_dir must be an absolute path"));
        }
        Ok(config)
    }

    /// Resolve the configured database or the default install-dir file.
    pub fn db_file(&self) -> AppResult<PathBuf> {
        match &self.db_file {
            Some(path) => Ok(path.clone()),
            None => Ok(install_dir()?.join("wsjump.db")),
        }
    }

    /// Default query order when the command does not override it.
    pub fn default_sort(&self) -> SortOrder {
        self.default_sort.unwrap_or_default()
    }

    /// Build a config for real-tree tests in core and sibling crates.
    #[doc(hidden)]
    pub fn for_test(root: &Path) -> Self {
        Self {
            root_dir: root.to_path_buf(),
            db_file: None,
            default_sort: None,
        }
    }

    /// Build an isolated index configuration for integration-style tests.
    #[doc(hidden)]
    pub fn for_test_with_db(root: &Path, database: &Path) -> Self {
        Self {
            root_dir: root.to_path_buf(),
            db_file: Some(database.to_path_buf()),
            default_sort: None,
        }
    }
}

/// Path of the running executable, with a uniform error message.
fn invoked_exe() -> AppResult<PathBuf> {
    std::env::current_exe()
        .map_err(|error| AppError::runtime(format!("cannot locate executable: {}", error)))
}

/// Directory containing the invoked executable, including a Windows bin symlink.
#[cfg(windows)]
fn exe_dir() -> AppResult<PathBuf> {
    invoked_exe()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| AppError::runtime("executable has no parent directory"))
}

/// Locate the install directory: the one holding `wsjump.toml` and `wsjump.db`.
///
/// On Windows this is simply the executable's directory. `current_exe()`
/// reports the *invoked* path there -- a copy in `bin\`, or a symlink/junction
/// pointing into `target\<profile>\` -- so the spelled install directory is
/// exactly what the caller sees, and nothing else needs probing.
#[cfg(windows)]
fn install_dir() -> AppResult<PathBuf> {
    exe_dir()
}

/// Locate the install directory: the one holding `wsjump.toml` and `wsjump.db`.
///
/// POSIX resolves symlinks before the process starts, so `current_exe()` is the
/// *real* binary -- for a `~/bin/wsjump -> target/release/wsjump` deployment
/// that is the build tree, not the install directory. The spelled path has to
/// be recovered from the environment instead: the directory part of `argv[0]`
/// when the caller used one, then `$PATH` entries that hold a `wsjump` or
/// `wsjump-tui` resolving back to this binary, and the executable's own
/// directory only as a last resort (a directly executed
/// `target/release/wsjump` with its config beside it).
#[cfg(unix)]
fn install_dir() -> AppResult<PathBuf> {
    let exe = invoked_exe()?;
    let argv0 = std::env::args_os().next();
    let path_var = std::env::var_os("PATH");
    let candidates = posix_candidates(&exe, argv0.as_deref(), path_var.as_deref());
    select_config_dir(&candidates)
}

/// Ordered install-directory candidates for a POSIX deployment, best first.
///
/// Split out from [`install_dir`] so the ordering can be tested without
/// touching the process environment.
#[cfg(unix)]
fn posix_candidates(exe: &Path, argv0: Option<&OsStr>, path_var: Option<&OsStr>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut push = |dir: Option<&Path>| {
        if let Some(dir) = dir {
            let dir = dir.to_path_buf();
            if !candidates.contains(&dir) {
                candidates.push(dir);
            }
        }
    };

    // argv[0] keeps the *spelled* path; a bare command name carries no
    // directory, which is exactly the case the PATH scan below covers.
    if let Some(argv0) = argv0 {
        let spelled = PathBuf::from(argv0);
        if spelled.components().count() > 1 {
            push(spelled.parent());
        }
    }

    // A bare `wsjump` reached us through the PATH; look for the entry whose
    // `wsjump` / `wsjump-tui` resolves back to this binary.
    if let (Some(path_var), Ok(real_exe)) = (path_var, exe.canonicalize()) {
        for dir in std::env::split_paths(path_var) {
            let hit = ["wsjump", "wsjump-tui"].iter().any(|name| {
                dir.join(name)
                    .canonicalize()
                    .map(|candidate| candidate == real_exe)
                    .unwrap_or(false)
            });
            if hit {
                push(Some(&dir));
            }
        }
    }

    // Last resort: the real binary's own directory.
    push(exe.parent());
    candidates
}

/// Pick the first candidate that actually holds `wsjump.toml`.
///
/// Precedence is the candidate order above. When several candidates qualify the
/// first one wins but the rest are reported, so a stray copy left in the build
/// directory can never take over silently.
#[cfg(unix)]
fn select_config_dir(candidates: &[PathBuf]) -> AppResult<PathBuf> {
    let found: Vec<&PathBuf> = candidates
        .iter()
        .filter(|dir| dir.join(CONFIG_FILE_NAME).is_file())
        .collect();
    match found.split_first() {
        Some((first, rest)) => {
            if !rest.is_empty() {
                let others = rest
                    .iter()
                    .map(|dir| dir.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                eprintln!(
                    "wsjump: warning: {} found in several directories; using '{}' (also in: {})",
                    CONFIG_FILE_NAME,
                    first.display(),
                    others
                );
            }
            Ok((*first).clone())
        }
        None => {
            let looked = candidates
                .iter()
                .map(|dir| dir.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            Err(AppError::runtime(format!(
                "no {} found in: {}",
                CONFIG_FILE_NAME, looked
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v3_config_requires_an_absolute_root_and_new_sort_name() {
        let root = std::env::temp_dir().to_string_lossy().replace('\\', "/");
        let config =
            Config::parse(&format!("root_dir = {root:?}\ndefault_sort = \"name\"")).unwrap();
        assert_eq!(config.default_sort(), SortOrder::Name);
        assert!(Config::parse("root_dir = \"relative\"").is_err());
        assert!(Config::parse("workspaces_dir = \"G:/ws/w\"\nutc_offset_hours = 8").is_err());
        assert!(Config::parse(&format!("root_dir = {root:?}\nutc_offset_hours = 8")).is_err());
        assert!(Config::parse(&format!("root_dir = {root:?}\ndefault_sort = \"id\"")).is_err());
    }

    /// A `bin/wsjump -> target/release/wsjump` deployment must resolve to `bin`,
    /// because that is where the config lives; the real binary's directory is
    /// only a fallback for a directly executed build.
    #[cfg(unix)]
    #[test]
    fn the_spelled_install_dir_outranks_the_real_binary_directory() {
        use crate::test_support::TempTree;
        use std::os::unix::fs::symlink;

        let tree = TempTree::new("install-dir");
        let real = tree.dir("target/release");
        let bin = tree.dir("bin");
        let exe = real.join("wsjump");
        std::fs::write(&exe, b"binary").unwrap();
        symlink(&exe, bin.join("wsjump")).unwrap();

        let path_var = bin.as_os_str();
        let candidates = posix_candidates(&exe, Some(OsStr::new("wsjump")), Some(path_var));
        assert_eq!(candidates, vec![bin.clone(), real.clone()]);

        // An argv[0] carrying a directory is used as spelled, without probing.
        let spelled = posix_candidates(
            &exe,
            Some(OsStr::new("/opt/ws/bin/wsjump")),
            Some(path_var),
        );
        assert_eq!(spelled.first(), Some(&PathBuf::from("/opt/ws/bin")));
    }

    /// A stray `wsjump.toml` in the build directory must not silently take over.
    #[cfg(unix)]
    #[test]
    fn the_first_candidate_holding_a_config_wins() {
        use crate::test_support::TempTree;

        let tree = TempTree::new("install-pick");
        let bin = tree.dir("bin");
        let real = tree.dir("target/release");
        tree.write("bin/wsjump.toml", "root_dir = \"/\"\n");
        assert_eq!(select_config_dir(&[bin.clone(), real.clone()]).unwrap(), bin);

        tree.write("target/release/wsjump.toml", "root_dir = \"/\"\n");
        assert_eq!(select_config_dir(&[bin.clone(), real.clone()]).unwrap(), bin);

        assert!(select_config_dir(&[tree.root.join("nowhere")]).is_err());
    }
}
