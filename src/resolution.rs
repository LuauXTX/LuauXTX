//! Native, filesystem-only module resolution. No package-manager process or hooks.
use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use serde_json::Value;

type ResolveResult = Result<PathBuf, String>;
const IMPORT_MAP_LIMIT: u64 = 2 * 1024 * 1024;

pub(crate) fn resolve_module(base: &Path, specifier: &str) -> ResolveResult {
    let requested = Path::new(specifier);
    let relative = specifier.starts_with("./")
        || specifier.starts_with("../")
        || (cfg!(windows) && (specifier.starts_with(".\\") || specifier.starts_with("..\\")));
    if !requested.is_absolute() && !relative {
        return resolve_package(base, specifier);
    }
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        base.join(requested)
    };
    let options = [
        candidate.clone(),
        candidate.with_extension("luau"),
        candidate.with_extension("lua"),
        candidate.join("init.luau"),
        candidate.join("init.lua"),
    ];
    for option in options {
        if option.is_file() {
            return canonical_file(&option);
        }
    }
    Err(format!(
        "cannot resolve module {specifier:?} from {}",
        base.display()
    ))
}

fn canonical_file(path: &Path) -> ResolveResult {
    fs::canonicalize(path)
        .map_err(|error| format!("cannot canonicalize module {}: {error}", path.display()))
}

fn package_name(name: &str) -> bool {
    let parts: Vec<_> = name.split('/').collect();
    name.len() <= 120
        && (1..=2).contains(&parts.len())
        && parts.iter().all(|part| {
            let stem = part.to_ascii_lowercase();
            !part.is_empty()
                && part.len() <= 60
                && part.as_bytes()[0].is_ascii_alphanumeric()
                && part
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
                && !matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
                && !(stem.len() == 4
                    && (stem.starts_with("com") || stem.starts_with("lpt"))
                    && stem.as_bytes()[3].is_ascii_digit())
        })
}

// Treat map paths identically on Unix and Windows: no traversal, drive/UNC paths,
// alternate data streams, or platform-specific separators. XTPKG emits '/' paths.
fn safe_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with([' ', '.'])
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.+@ ()-".contains(&c))
                && !matches!(
                    part.split('.')
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str(),
                    "con" | "prn" | "aux" | "nul"
                )
                && !{
                    let stem = part.split('.').next().unwrap_or("").to_ascii_lowercase();
                    stem.len() == 4
                        && (stem.starts_with("com") || stem.starts_with("lpt"))
                        && stem.as_bytes()[3].is_ascii_digit()
                }
        })
}

fn resolve_package(base: &Path, name: &str) -> ResolveResult {
    if !package_name(name) {
        return Err(format!(
            "invalid package name {name:?}; expected name or scope/package (use ./ or ../ for relative imports)"
        ));
    }
    let base = if base.is_absolute() {
        base.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| format!("cannot resolve package {name:?}: {error}"))?
            .join(base)
    };
    let base = fs::canonicalize(&base).map_err(|error| {
        format!(
            "cannot resolve package {name:?} from {}: {error}",
            base.display()
        )
    })?;
    for ancestor in base.ancestors() {
        let packages = ancestor.join("packages");
        let map_path = packages.join(".xtpkg-imports.json");
        let file = match fs::File::open(&map_path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "cannot read import map {} for package {name:?}: {error}",
                    map_path.display()
                ));
            }
        };
        let resolve = || -> Result<Option<PathBuf>, String> {
            let mut bytes = Vec::new();
            file.take(IMPORT_MAP_LIMIT + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| format!("cannot read import map: {error}"))?;
            if bytes.len() as u64 > IMPORT_MAP_LIMIT {
                return Err("import map exceeds 2 MiB limit".into());
            }
            let map: Value = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid import map JSON: {error}"))?;
            let map = map.as_object().ok_or("import map must be a JSON object")?;
            let Some(record) = map.get(name) else {
                return Ok(None);
            };
            let directory = record
                .get("path")
                .and_then(Value::as_str)
                .ok_or("package record needs a string path")?;
            let directory = directory.strip_prefix("./").unwrap_or(directory);
            if !safe_path(directory) {
                return Err(
                    "invalid package path; expected a safe relative path inside packages/".into(),
                );
            }
            let entry = record
                .get("entry")
                .and_then(Value::as_str)
                .ok_or("invalid entrypoint; package record needs a string entry")?;
            if !safe_path(entry) || !(entry.ends_with(".lua") || entry.ends_with(".luau")) {
                return Err(
                    "invalid entrypoint; expected a safe relative .lua or .luau file".into(),
                );
            }
            let packages = canonical_file(&packages)?;
            let directory = canonical_file(&packages.join(directory))?;
            if !directory.starts_with(&packages) || !directory.is_dir() {
                return Err("package path must name a directory inside packages/".into());
            }
            let requested = directory.join(entry);
            let resolved = fs::canonicalize(&requested)
                .map_err(|error| format!("invalid entrypoint {}: {error}", requested.display()))?;
            if !resolved.starts_with(&directory) || !resolved.is_file() {
                return Err(format!(
                    "invalid entrypoint {}; must be a file inside the package directory",
                    requested.display()
                ));
            }
            Ok(Some(resolved))
        };
        match resolve() {
            Ok(None) => continue,
            Ok(Some(path)) => return Ok(path),
            Err(error) => {
                return Err(format!(
                    "cannot resolve package {name:?} using {}: {error}",
                    map_path.display()
                ));
            }
        }
    }
    Err(format!(
        "package {name:?} is not installed; searched packages/.xtpkg-imports.json from {} to the filesystem root",
        base.display()
    ))
}
