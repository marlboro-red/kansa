# kansa

Turns a PM's prose HLD (markdown in a GitHub repo) into a structured, traceable requirement
inventory — without ever editing the HLD. Exports to [reqtrace](https://github.com/marlboro-red/reqtrace).
See [spec.md](spec.md).

```
crates/kansa-core   state store, objects, segmentation, export — all logic
crates/kansa-cli    `kansa` CLI (thin)
app/                Tauri 2 + SolidJS desktop app (thin)
```

## Dev

```sh
cargo test                       # core + cli
cd app && npm install && npm run tauri dev   # desktop app
```

Browser dev loop (frontend in Chrome, real core over HTTP):

```sh
cargo run -p kansa-cli -- serve  # http://127.0.0.1:1430
cd app && npx vite               # http://localhost:1420
```

State lives under `$KANSA_HOME` (default: OS config dir `/kansa`). GitHub access goes through `gh`.
