//! Shared LuauXTX runtime entrypoints.
//!
//! Native host integrations live in `native`; browser hosts use `run` through
//! the `web` crate. This keeps the VM dependency and bundled Luau sources in
//! one package while platform bindings remain target-specific.

#[cfg(not(target_arch = "wasm32"))]
pub mod native;

#[cfg(not(target_arch = "wasm32"))]
mod resolution;

#[cfg(not(target_arch = "wasm32"))]
mod gfx;

#[cfg(not(target_arch = "wasm32"))]
pub use native::cli_main;

/// Execute a standalone Luau chunk using the embedded Luau VM.
///
/// Browser bindings install their own host APIs around this same VM. Values are
/// rendered as strings so this minimal portable entrypoint has no JS-specific
/// types in its public API.
pub fn run(source: &str) -> Result<String, String> {
    let lua = mlua::Lua::new();
    let native = lua.create_table().map_err(|error| error.to_string())?;
    run_with_host(&lua, source, native)
}

mod standard_library {
    include!(concat!(env!("OUT_DIR"), "/standard_library.rs"));
}

/// Execute Luau with embedded standard-library modules and caller-supplied host bindings.
pub fn run_with_host(lua: &mlua::Lua, source: &str, native: mlua::Table) -> Result<String, String> {
    let cache = lua.create_table().map_err(|error| error.to_string())?;
    let require = lua.create_function(move |lua, name: String| {
        let name = name.strip_prefix("@luauxtx/").unwrap_or(&name);
        let cached: mlua::Value = cache.raw_get(name)?;
        if !matches!(cached, mlua::Value::Nil) { return Ok(cached); }
        let module = standard_library::STANDARD_LIBRARY.iter().find_map(|(module, source)| (*module == name).then_some(*source))
            .ok_or_else(|| mlua::Error::RuntimeError(format!("unknown built-in module {name:?}")))?;
        let value = lua.load(module).set_name(format!("@luauxtx/{name}")).call::<mlua::Value>(native.clone())?;
        cache.raw_set(name, value.clone())?;
        Ok(value)
    }).map_err(|error| error.to_string())?;
    lua.globals().set("require", require).map_err(|error| error.to_string())?;
    let value: mlua::Value = lua.load(source).eval().map_err(|error| error.to_string())?;
    match value {
        mlua::Value::Nil => Ok(String::new()),
        mlua::Value::String(value) => value.to_str().map(|value| value.to_owned()).map_err(|error| error.to_string()),
        value => Ok(format!("{value:?}")),
    }
}

#[cfg(test)]
mod namespace_tests {
    #[test]
    fn portable_host_uses_the_same_native_namespace_and_cache() {
        assert_eq!(super::run("local t = require('@luauxtx/async/task'); assert(t == require('async/task')); return 'ok'").unwrap(),"ok");
        assert!(super::run("require('@luauxtx/missing')").unwrap_err().contains("unknown built-in module"));
    }
}
