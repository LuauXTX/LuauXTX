//! Exercise the real native CLI and the exact import-map/wrapper layout emitted by XTPKG.
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use uuid::Uuid;

struct Project {
    root: PathBuf,
    project: PathBuf,
}

impl Project {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("luauxtx packages 雪 {}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(root.join("unrelated cwd")).unwrap();
        Self { root, project }
    }

    fn package(&self, project: &Path, name: &str, entry: &str, source: &str) {
        let packages = project.join("packages");
        let target = packages.join(name).join(entry);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, source).unwrap();
        let map_path = packages.join(".xtpkg-imports.json");
        let mut map = fs::read(&map_path)
            .ok()
            .map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap())
            .unwrap_or(json!({}));
        map[name] = json!({ "path": format!("./{name}"), "entry": entry, "version": "1.0.0", "revision": "a".repeat(40) });
        fs::write(map_path, serde_json::to_vec_pretty(&map).unwrap()).unwrap();
        let basename = name.rsplit('/').next().unwrap();
        fs::write(
            packages.join(format!("{name}.luau")),
            format!(
                "return require({})",
                quote(&format!("./{basename}/{entry}"))
            ),
        )
        .unwrap();
    }

    fn run(&self, entry: &str, source: &str) -> Output {
        let script = self.project.join(entry);
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_luauxtx"))
            .arg(script)
            .current_dir(self.root.join("unrelated cwd"))
            // A package manager, Git, shell, and install hooks are not needed.
            .env("PATH", "")
            .output()
            .unwrap()
    }

    fn success(&self, entry: &str, source: &str) {
        let output = self.run(entry, source);
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn failure(&self, source: &str, expected: &str) {
        let output = self.run("src/main.luau", source);
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains(expected),
            "expected {expected:?}, got {error}"
        );
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn quote(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

#[test]
fn loads_actual_xtpkg_map_without_adapters_wrappers_or_package_manager() {
    let p = Project::new();
    p.package(
        &p.project,
        "xtest",
        "lib/custom.lua",
        "return { answer = 42 }",
    );
    fs::remove_file(p.project.join("packages/xtest.luau")).unwrap();
    p.success(
        "src/deep/main.luau",
        "assert(require('xtest').answer == 42)",
    );
}

#[test]
fn loads_scoped_packages_and_lua_entrypoints() {
    let p = Project::new();
    p.package(
        &p.project,
        "scope/package",
        "src/main.luau",
        "return { scoped = true }",
    );
    p.package(&p.project, "plain", "init.lua", "return 'plain'");
    p.success("app.luau", "assert(require('scope/package').scoped); assert(require('plain') == 'plain'); assert(require('scope/package') == require('./packages/scope/package'))");
}

#[test]
fn nearer_installation_wins_and_absent_names_walk_up() {
    let p = Project::new();
    p.package(&p.project, "shared", "main.luau", "return 'parent'");
    p.package(&p.project, "parentonly", "main.luau", "return 'fallback'");
    p.package(
        &p.project.join("sub"),
        "shared",
        "different.luau",
        "return 'nearest'",
    );
    p.success(
        "sub/src/main.luau",
        "assert(require('shared') == 'nearest'); assert(require('parentonly') == 'fallback')",
    );
}

#[test]
fn invalid_nearer_installation_never_falls_back_to_parent_version() {
    let p = Project::new();
    p.package(&p.project, "shared", "main.luau", "return 'parent'");
    p.package(
        &p.project.join("src"),
        "shared",
        "main.luau",
        "return 'nearest'",
    );
    fs::remove_file(p.project.join("src/packages/shared/main.luau")).unwrap();
    p.failure("require('shared')", "invalid entrypoint");
}

#[test]
fn nested_dependencies_keep_their_context_after_load_and_in_callbacks() {
    let p = Project::new();
    p.package(&p.project, "dep", "main.luau", "return { version = 1 }");
    p.package(
        &p.project,
        "outer",
        "src/main.luau",
        "return function() return require('scope/dep'), require('./sibling') end",
    );
    p.package(
        &p.project.join("packages/outer"),
        "scope/dep",
        "main.luau",
        "return { version = 2 }",
    );
    fs::write(
        p.project.join("packages/outer/src/sibling.luau"),
        "return 42",
    )
    .unwrap();
    p.success("app.luau", "local outer = require('outer'); local parent = require('dep'); local nested, sibling = outer(); assert(parent.version == 1 and nested.version == 2 and sibling == 42); assert(outer() == nested); local called = false; task.defer(function() local n, s = outer(); called = n == nested and s == 42 end); task.wait(0.01); assert(called)");
}

#[test]
fn flattened_dependencies_and_siblings_resolve_from_package_sources() {
    let p = Project::new();
    p.package(&p.project, "dep", "main.luau", "return { version = 1 }");
    p.package(
        &p.project,
        "outer",
        "src/main.luau",
        "return require('dep')",
    );
    p.success("app.luau", "assert(require('outer') == require('dep'))");
}

#[test]
fn canonical_cache_is_shared_by_bare_wrapper_relative_and_absolute_imports() {
    let p = Project::new();
    p.package(
        &p.project,
        "widget",
        "src/main.luau",
        "executions = (executions or 0) + 1; return {}",
    );
    let absolute = quote(
        p.project
            .join("packages/widget/src/main.luau")
            .to_str()
            .unwrap(),
    );
    p.success("app.luau", &format!("local m = require('widget'); assert(m == require('./packages/widget')); assert(m == require('./packages/widget/src/../src/main.luau')); assert(m == require({absolute})); assert(executions == 1)"));
}

#[test]
fn nil_and_false_exports_are_cached() {
    let p = Project::new();
    p.package(
        &p.project,
        "nothing",
        "main.luau",
        "nilLoads = (nilLoads or 0) + 1; return nil",
    );
    p.package(
        &p.project,
        "negative",
        "main.luau",
        "falseLoads = (falseLoads or 0) + 1; return false",
    );
    p.success("app.luau", "assert(require('nothing') == nil and require('./packages/nothing/main.luau') == nil); assert(require('negative') == false and require('./packages/negative') == false); assert(nilLoads == 1 and falseLoads == 1)");
}

#[test]
fn builtins_take_precedence_and_explicit_local_paths_still_work() {
    let p = Project::new();
    p.package(
        &p.project,
        "system/fs",
        "main.luau",
        "error('shadowed builtin')",
    );
    p.package(
        &p.project,
        "process",
        "main.luau",
        "error('shadowed process')",
    );
    p.package(&p.project, "localmodule", "main.luau", "return 'package'");
    fs::write(p.project.join("localmodule.luau"), "return 'local'").unwrap();
    fs::create_dir_all(p.project.join("folder")).unwrap();
    fs::write(p.project.join("folder/init.lua"), "return 3").unwrap();
    p.success("app.luau", "assert(require('system/fs') == fs); assert(require('process') == process); assert(require('./localmodule') == 'local'); assert(require('localmodule') == 'package'); assert(require('./folder') == 3)");
}

#[test]
fn cycles_are_reported_and_failed_loads_can_be_retried() {
    let p = Project::new();
    p.package(
        &p.project,
        "cycle",
        "main.luau",
        "return require('./sub/../main')",
    );
    fs::create_dir_all(p.project.join("packages/cycle/sub")).unwrap();
    p.package(
        &p.project,
        "retry",
        "main.luau",
        "attempts = (attempts or 0) + 1; if attempts == 1 then error('try again') end; return 42",
    );
    p.success("app.luau", "local ok, err = pcall(require, 'cycle'); assert(not ok and tostring(err):find('circular require')); assert(not pcall(require, 'retry')); assert(require('retry') == 42 and attempts == 2)");
}

#[test]
fn missing_names_are_clear_and_never_load_an_unmapped_local_file() {
    let p = Project::new();
    fs::write(p.project.join("missing.luau"), "error('must not run')").unwrap();
    p.failure("require('missing')", "package \"missing\" is not installed");
    for name in [
        "@scope/pkg",
        "foo/bar/baz",
        "scope/../outside",
        "scope\\pkg",
        "",
        "CON",
        "lpt1",
        "Uppercase",
    ] {
        p.failure(&format!("require({})", quote(name)), "invalid package name");
    }
}

#[test]
fn malformed_maps_and_invalid_entrypoints_have_context() {
    let p = Project::new();
    p.package(&p.project, "bad", "main.luau", "return 1");
    let map_path = p.project.join("packages/.xtpkg-imports.json");
    for (contents, message) in [
        ("{".to_string(), "invalid import map JSON"),
        ("[]".to_string(), "import map must be a JSON object"),
        (json!({"bad": "wrong"}).to_string(), "string path"),
        (
            json!({"bad": {"path": "./bad"}}).to_string(),
            "invalid entrypoint",
        ),
        (
            json!({"bad": {"path": "../outside", "entry": "main.luau"}}).to_string(),
            "invalid package path",
        ),
        (
            json!({"bad": {"path": "./bad", "entry": "../outside.luau"}}).to_string(),
            "invalid entrypoint",
        ),
        (
            json!({"bad": {"path": "./bad", "entry": "main.js"}}).to_string(),
            "invalid entrypoint",
        ),
        (
            json!({"bad": {"path": "./bad", "entry": "absent.luau"}}).to_string(),
            "invalid entrypoint",
        ),
        (" ".repeat(2 * 1024 * 1024 + 1), "exceeds 2 MiB"),
    ] {
        fs::write(&map_path, contents).unwrap();
        p.failure("require('bad')", message);
        p.failure("require('bad')", ".xtpkg-imports.json");
    }
}

#[test]
fn map_paths_cannot_use_platform_specific_traversal_or_device_aliases() {
    let p = Project::new();
    p.package(&p.project, "bad", "main.luau", "return 1");
    let map_path = p.project.join("packages/.xtpkg-imports.json");
    for path in [
        "/tmp",
        "C:/temp",
        "C:temp",
        "\\\\server\\share",
        "bad/../bad",
        "bad\\..\\bad",
        "bad//sub",
        "bad/CON.txt",
        "bad/lpt1.lua",
        "bad/sub.",
    ] {
        fs::write(
            &map_path,
            json!({"bad": {"path": path, "entry": "main.luau"}}).to_string(),
        )
        .unwrap();
        p.failure("require('bad')", "invalid package path");
    }
    for entry in [
        "/tmp/file.lua",
        "C:/file.lua",
        "..\\escape.lua",
        "main.lua:stream",
        "NUL.lua",
        "sub//main.lua",
    ] {
        fs::write(
            &map_path,
            json!({"bad": {"path": "./bad", "entry": entry}}).to_string(),
        )
        .unwrap();
        p.failure("require('bad')", "invalid entrypoint");
    }
}

#[cfg(windows)]
#[test]
fn windows_separator_case_and_verbatim_paths_share_the_cache() {
    let p = Project::new();
    p.package(
        &p.project,
        "widget",
        "src/main.luau",
        "windowsLoads = (windowsLoads or 0) + 1; return {}",
    );
    let path = p.project.join("packages/widget/src/main.luau");
    let uppercase = quote(&path.to_str().unwrap().to_uppercase());
    let verbatim = quote(fs::canonicalize(path).unwrap().to_str().unwrap());
    p.success("app.luau", &format!(r#"local m = require('widget'); assert(m == require('.\\packages\\widget\\src\\main.luau')); assert(m == require({uppercase})); assert(m == require({verbatim})); assert(windowsLoads == 1)"#));
}

#[test]
fn symlinks_share_the_cache_and_mapped_entries_cannot_escape() {
    let p = Project::new();
    p.package(
        &p.project,
        "widget",
        "main.luau",
        "linkLoads = (linkLoads or 0) + 1; return {}",
    );
    let target = p.project.join("packages/widget/main.luau");
    let alias = p.project.join("alias.luau");
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(&target, &alias);
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_file(&target, &alias);
    if let Err(error) = result {
        #[cfg(windows)]
        if error.raw_os_error() == Some(1314) {
            eprintln!(
                "file symlinks need Developer Mode or privilege; skipping symlink assertions"
            );
            return;
        }
        panic!("cannot create symlink: {error}");
    }
    p.success(
        "app.luau",
        "assert(require('widget') == require('./alias')); assert(linkLoads == 1)",
    );
    fs::write(
        p.project.join("outside.luau"),
        "error('must not execute outside package')",
    )
    .unwrap();
    fs::remove_file(&target).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(p.project.join("outside.luau"), &target).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(p.project.join("outside.luau"), &target).unwrap();
    p.failure("require('widget')", "inside the package directory");
}
