//! A child-only PATH for native CLIs launched by a macOS application.
//! Initialization performs bounded filesystem work and must run on a worker.
//! No shell profile, credential file, or global environment is read or changed.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const INHERITED_LIMIT: usize = 128;
const MANAGER_ENTRY_LIMIT: usize = 64;
const MANAGER_BIN_LIMIT: usize = 12;
const CACHE_TTL: Duration = Duration::from_secs(60);
const CACHE_HOMES: usize = 8;

struct CachedPath {
    built_at: Instant,
    directories: Vec<PathBuf>,
}
static PATHS: OnceLock<Mutex<HashMap<PathBuf, CachedPath>>> = OnceLock::new();

/// Return a cached, validated PATH directory list. Call from a worker only.
/// Demand-driven expiry discovers new installations without recurring polling.
pub fn dirs(home: &Path) -> Vec<PathBuf> {
    let cache = PATHS.get_or_init(|| Mutex::new(HashMap::new()));
    let home = home.canonicalize().unwrap_or_else(|_| home.to_owned());
    let mut cached = cache.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(entry) = cached.get(&home)
        && entry.built_at.elapsed() < CACHE_TTL
    {
        return entry.directories.clone();
    }
    let directories = build_dirs(&home, std::env::var_os("PATH").as_deref());
    if !cached.contains_key(&home) && cached.len() >= CACHE_HOMES {
        // Only the current user's entry is normally needed. Bound fixture and
        // alternate-home caching without maintaining an independent timer.
        if let Some(oldest) = cached
            .iter()
            .min_by_key(|(_, entry)| entry.built_at)
            .map(|(home, _)| home.clone())
        {
            cached.remove(&oldest);
        }
    }
    cached.insert(
        home,
        CachedPath {
            built_at: Instant::now(),
            directories: directories.clone(),
        },
    );
    directories
}

/// Preserve inherited settings/authentication; override PATH on this child only.
/// Call when constructing a child process on a worker, after choosing its CLI.
pub fn apply(command: &mut Command) {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    apply_dirs(command, &dirs(&home));
}

fn apply_dirs(command: &mut Command, directories: &[PathBuf]) {
    let executable = PathBuf::from(command.get_program());
    let mut candidates = Vec::with_capacity(directories.len() + 2);
    if let Some(parent) = executable.parent().filter(|path| path.is_absolute()) {
        candidates.push(parent.to_owned());
    }
    // A versioned Cursor shim can carry its own runtime next to the target.
    if let Ok(target) = executable.canonicalize()
        && let Some(parent) = target.parent()
    {
        candidates.push(parent.to_owned());
    }
    candidates.extend_from_slice(directories);
    let validated = validated_dirs(candidates);
    if let Ok(path) = std::env::join_paths(validated) {
        command.env("PATH", path);
    }
}

fn build_dirs(home: &Path, inherited: Option<&OsStr>) -> Vec<PathBuf> {
    let mut candidates: Vec<_> = inherited
        .map(|path| std::env::split_paths(path).take(INHERITED_LIMIT).collect())
        .unwrap_or_default();
    candidates.extend([
        home.join(".local/bin"),
        home.join(".cargo/bin"),
        home.join(".opencode/bin"),
        home.join(".npm-global/bin"),
        home.join(".claude/local"),
        home.join(".grok/bin"),
        home.join(".cursor/bin"),
        home.join(".antigravity/bin"),
        home.join("bin"),
        home.join(".volta/bin"),
        home.join(".bun/bin"),
        home.join(".asdf/shims"),
        home.join(".nodenv/shims"),
        home.join(".local/share/mise/shims"),
        home.join(".nvm/current/bin"),
        home.join(".fnm/aliases/default/bin"),
        home.join(".local/share/fnm/aliases/default/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        PathBuf::from("/opt/homebrew/opt/node/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/local/sbin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ]);
    for (root, suffix) in [
        (home.join(".nvm/versions/node"), "bin"),
        (home.join(".fnm/node-versions"), "installation/bin"),
        (
            home.join(".local/share/fnm/node-versions"),
            "installation/bin",
        ),
        (home.join(".local/share/mise/installs/node"), "bin"),
        (home.join(".local/share/cursor-agent/versions"), ""),
    ] {
        candidates.extend(manager_bins(&root, suffix));
    }
    validated_dirs(candidates)
}

fn manager_bins(root: &Path, suffix: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut versions: Vec<_> = entries
        .take(MANAGER_ENTRY_LIMIT)
        .flatten()
        .filter(|entry| {
            entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() || kind.is_symlink())
        })
        .map(|entry| entry.path())
        .collect();
    versions.sort_unstable_by(|a, b| version_key(b).cmp(&version_key(a)).then(b.cmp(a)));
    versions
        .into_iter()
        .take(MANAGER_BIN_LIMIT)
        .map(|version| version.join(suffix))
        .collect()
}

fn version_key(path: &Path) -> Vec<u64> {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn validated_dirs(candidates: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|path| {
            path.is_absolute()
                && std::env::join_paths(std::iter::once(path)).is_ok()
                && path.is_dir()
                && !path.as_os_str().is_empty()
                && seen.insert(path.canonicalize().unwrap_or_else(|_| path.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("orbit-process-env-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn executable(&self, path: &Path, contents: &str) {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn launch_services_path_executes_real_env_interpreter_shim_and_preserves_child_env() {
        let fixture = Fixture::new();
        let bin = fixture.0.join(".local/bin");
        let interpreter = bin.join("orbit-fixture-node");
        let cli = fixture.0.join("selected-cli/codex");
        fixture.executable(
            &interpreter,
            "#!/bin/sh\nprintf 'interpreter-ok:%s' \"$ORBIT_FIXTURE_SETTING\"\n",
        );
        fixture.executable(
            &cli,
            "#!/usr/bin/env orbit-fixture-node\nignored JavaScript fixture\n",
        );
        let mut command = Command::new(&cli);
        command.env("ORBIT_FIXTURE_SETTING", "preserved");
        command.env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        let directories = build_dirs(
            &fixture.0,
            Some(OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin")),
        );
        apply_dirs(&mut command, &directories);
        let output = command.output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"interpreter-ok:preserved");
        let child_path = command
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("PATH"))
            .unwrap()
            .1
            .unwrap();
        assert_eq!(
            std::env::split_paths(child_path).next().unwrap(),
            cli.parent().unwrap()
        );
    }

    #[test]
    fn validated_inherited_paths_reject_relative_missing_entries_and_deduplicate_aliases() {
        let fixture = Fixture::new();
        let first = fixture.0.join("bin");
        fs::create_dir_all(&first).unwrap();
        let alias = fixture.0.join("alias");
        symlink(&first, &alias).unwrap();
        let inherited = std::env::join_paths([
            first.clone(),
            alias,
            PathBuf::from("."),
            fixture.0.join("missing"),
            first.clone(),
        ])
        .unwrap();
        let directories = build_dirs(&fixture.0, Some(&inherited));
        assert_eq!(directories.first(), Some(&first));
        assert_eq!(
            directories
                .iter()
                .filter(|path| path.canonicalize().ok().as_ref() == Some(&first))
                .count(),
            1
        );
        assert!(
            directories
                .iter()
                .all(|path| path.is_absolute() && path.is_dir())
        );
    }

    #[test]
    fn manager_discovery_is_bounded_and_cache_reuses_the_resolved_path() {
        let fixture = Fixture::new();
        let manager = fixture.0.join(".nvm/versions/node");
        for index in 0..100 {
            fs::create_dir_all(manager.join(format!("v{index}.0.0/bin"))).unwrap();
        }
        let found = manager_bins(&manager, "bin");
        assert!(found.len() <= MANAGER_BIN_LIMIT);
        assert!(found.iter().all(|path| path.is_dir()));
        let first = dirs(&fixture.0);
        fs::create_dir_all(fixture.0.join(".volta/bin")).unwrap();
        let second = dirs(&fixture.0);
        assert_eq!(first, second);
        assert!(!second.contains(&fixture.0.join(".volta/bin")));
    }
}
