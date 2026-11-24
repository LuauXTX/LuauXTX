# LuauXTX

LuauXTX is an early Node-style runtime for trusted Luau scripts. Rust hosts the
official Luau VM; Tokio provides its event loop and asynchronous system APIs.

## Standard library

Public runtime APIs are implemented in Luau under [`lib/`](lib/) and bundled
into the executable at compile time. Rust exposes private callback-based host
primitives to those modules only; user scripts receive the public `Promise`,
`task`, `fs`, and `http` APIs instead.

- [`lib/async/promise.luau`](lib/async/promise.luau) implements promise state and chaining.
- [`lib/async/task.luau`](lib/async/task.luau) wraps coroutine scheduling and timers.
- [`lib/system/fs.luau`](lib/system/fs.luau) turns filesystem completions into promises.
- [`lib/net/http/client.luau`](lib/net/http/client.luau) provides HTTP client requests.
- [`lib/net/http/server.luau`](lib/net/http/server.luau) provides a small HTTP server.

To add a built-in module, create a `.luau` file in `lib/` and rebuild. Module
names follow their path, so `lib/system/path.luau` is loaded with
`require("system/path")` and `lib/crypto/base64.luau` is loaded with
`require("crypto/base64")`.
Directory modules can use `init.luau`, such as `lib/json/init.luau` for
`require("json")`. The build rejects duplicate module names.

```luau
-- lib/system/path.luau
local path = {}

function path.join(left, right)
    return left .. "/" .. right
end

return path
```

```luau
local path = require("system/path")
print(path.join("src", "main.luau"))
```

## Run

```bash
cargo run -- examples/hello.luau world
```

## Browser build

Install `wasm-bindgen-cli`, then build the browser crate from the repository
root. On this MSYS2 setup Luau's C++ build needs the installed WASI sysroot:

```powershell
cargo install wasm-bindgen-cli
$env:LUAU_CXXFLAGS = "--target=wasm32-wasip1 --sysroot=D:/Toolchains/msys64/ucrt64/share/wasi-sysroot"
cargo build -p luauxtx-web --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir web/demo/pkg target/wasm32-unknown-unknown/release/luauxtx_web.wasm
cd web/demo; python -m http.server
```

Open the printed local URL. The demo executes Luau through the exported `run`
function and uses the browser DOM binding to update its button.

The executable accepts a script followed by script arguments. A standalone
separator is optional:

```bash
luauxtx app.luau -- production 3000
```

## Available APIs

```lua
print(process.argv[3])
print(process.cwd())
print(process.getenv("HOME"))

fs.write("message.txt", "hello"):await()
print(fs.read("message.txt"):await())

local config = require("./config") -- .luau, .lua, or directory/init.luau
```

`fs.read(path)` and `fs.write(path, contents)` return `Promise` objects and use
Tokio's asynchronous filesystem APIs. `fs.readSync` and `fs.writeSync` are
available when blocking behavior is intentional.

`fs.mkdir(path, recursive?)`, `fs.copy(source, destination, recursive?)`,
`fs.remove(path, recursive?)`, `fs.rename(source, destination)`,
`fs.symlink(target, path)`, `fs.readDir(path)`, and `fs.stat(path)` return
Promises. `stat` resolves to `{ isFile, isDirectory, size }`; `readDir` resolves
to entry names. Directory copy/removal requires `recursive = true`.
On Windows, `fs.rename` retries temporary sharing/access errors for up to 1.6
seconds, allowing package directory replacement to survive brief file locks.
It does not change permissions or delete the destination to force a rename.

`process.cwd()`, `process.chdir(path)`, and `process.exit(code?)` control the
current process. `process.spawn(command, args?, options?)` starts a program and
returns its PID. `process.exec(command, args?, options?)` waits and returns
`{ code, stdout, stderr }`. Arguments are arrays and are passed directly (no
shell); options may include `cwd` and `env = { NAME = "value" }`.

```lua
task.delay(0.5, function()
    print("later")
end)

local response = http.fetch("https://example.com"):await()
print(response.status, response.body)
```

`task.wait`, `task.spawn`, `task.defer`, and `task.delay` use the runtime event
loop. `Promise` supports `resolve`, `reject`, `all`, `andThen`, `catch`, and
`await`. HTTP currently provides an asynchronous `http.fetch(url)` GET client.

For a small HTTP server, use `require("net/http/server")`. `createServer` takes
a function (or a `net/http/router` router), and `listen(port)` begins accepting
connections on `127.0.0.1`. Request bodies are strings; `res:send`,
`res:json`, `res:status`, and `res:header` build the response.

`require("net/websocket")` supports both client and standalone server sockets.
`websocket.connect("ws://...")` (and `wss://`) returns a Promise for a socket;
use `socket:send`, `socket:sendBinary`, `socket:receive`, and `socket:close`.
`websocket.listen(port, handler)` accepts WebSocket connections on `127.0.0.1`.

`net/tcp` provides `connect(host, port)` and `listen(port, handler)`; connected
sockets expose Promise-based `send` and `receive`. `net/udp.bind(port)` returns
a datagram socket with `send(data, host, port)` and `receive`. `net/dns.lookup`
asynchronously resolves a hostname to an array of IP-address strings.

`require` caches module return values. Modules must return the value they
export. Built-in modules are namespaced: `require("system/fs")`,
`require("system/path")`, `require("net/http")`, `require("net/url")`,
`require("async/promise")`, and `require("async/task")`. `process`, `fs`,
`http`, `task`, and `Promise` remain available as globals.

The entry script may use `:await()`. Modules loaded by `require` execute
synchronously while loading, so asynchronous work belongs in exported
functions or in a promise returned by the module.

This runtime runs trusted local code. Its file APIs are not sandboxed.

## Installed packages (native CLI)

Install a dependency with [XTPKG](https://github.com/LuauXTX/xtpkg), then require
it by its package name:

```bash
xtpkg add xtest
luauxtx src/main.luau
```

```luau
local xtest = require("xtest")
local scoped = require("scope/package") -- after installing scope/package
```

No import adapter or generated wrapper path is needed. Starting at the importing
file's directory, the native runtime checks `packages/.xtpkg-imports.json` at
each directory and walks up to the filesystem root. The nearest map containing
the requested name wins. A nearer map without that name allows the search to
continue. A malformed map or a broken matching entry produces an error with the
map path and package name; it does not silently load a different parent version.
Resolution follows the importing file, even if the working directory changes.

XTPKG already generates maps with this format:

```json
{
  "xtest": {
    "path": "./xtest",
    "entry": "src/main.luau",
    "version": "1.0.0"
  },
  "scope/package": {
    "path": "./scope/package",
    "entry": "init.lua",
    "version": "1.0.0"
  }
}
```

`path` is a directory relative to the map's `packages/` directory, and `entry`
is an exact `.lua` or `.luau` file relative to that package directory. Version,
revision, and other metadata do not affect runtime resolution. Paths must stay
inside their package installation; traversal, absolute map paths, device names,
and symlink entries escaping the package directory are rejected. Import maps
are limited to 2 MiB. Package names use XTPKG's lowercase `name` or
`scope/package` syntax; arbitrary package subpaths and Node's `@scope` syntax
are not supported.

Both flattened XTPKG dependencies and nested installations work. For example,
`packages/outer/packages/.xtpkg-imports.json` can select a private dependency
for modules inside `outer`; otherwise they find the project's shared packages.
An exported function keeps its defining module's resolution context when
called later, including from a timer or callback.

Built-ins (including `process`) take precedence over package names. Relative
imports (`./module`, `../module`), absolute paths, `.lua`/`.luau` extension
lookup, and directory `init.lua`/`init.luau` imports continue to work. Existing
XTPKG wrappers such as `require("./packages/xtest")` also remain usable. Files
are cached by their canonical filesystem path, so bare imports, wrappers,
absolute paths, `..` paths, and symlinks share the entrypoint's export and run it
once per VM. Windows separator and case variants share that cache too. Failed
loads can be retried; circular imports report an error.

The loader only reads installed files. It does not invoke XTPKG, fetch missing
packages, or run installation hooks. Install packages explicitly before running
your script. Filesystem package loading belongs to the native CLI; the browser
host continues to expose embedded built-ins only.

## Tests

```bash
cargo test --locked -p luauxtx
```

The CLI integration suite covers package precedence, scoped names, nested and
flattened dependencies, deferred imports, canonical caching, map errors, and
portable path validation. CI runs the suite on Windows, Linux, and macOS.
File-symlink assertions on Windows require Developer Mode or symlink privilege;
the test reports a skip for those assertions if that privilege is unavailable.

## Next steps

- HTTP server and TCP APIs
- permissions and isolated script execution
