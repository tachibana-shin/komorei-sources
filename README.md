# Komorei Sources

A collection of content sources for the **Komorei** app, modeled after
[`Aidoku-Community/sources`](https://github.com/Aidoku-Community/sources): each
source is a `no_std` Rust crate compiled to `wasm32-unknown-unknown` and
packaged into a `.krx` file. Every source is built into a single source list
(`public/`) that can be added to the app as a repository.

> This is a **standalone git repository** — it lives next to the app repo at a
> relative path (like `komorei-sdk/`). The app only references
> `sources/sources/vi.ophim/package.krx` for JVM tests; source code does not
> belong to the app repo.

## Usage

On a device with Komorei installed (Sources tab → "Add repo"), add the
`<deployment>/index.min.json` URL of this repo to browse and install each
source.

```sh
# example: this repo deployed to GitHub Pages
# https://<user>.github.io/<repo>/index.min.json
```

## Layout

This repository follows the standard Aidoku-Community layout — every source
crate lives in a `sources/<id>/` subdirectory, where the folder name matches
the source `info.id` (e.g. `vi.ophim`):

```
.
├── .github/workflows/   CI: build the list on main, deploy public/ to gh-pages
├── sources/             one crate per source (Cargo.toml, src/, res/)
│   ├── vi.ophim/        OPhim API (classic ophim1.com + flat fork phimapi.com)
│   ├── vi.kkphim/       KKPhim HTML scrape (m3u8 links, configurable base URL)
│   ├── vi.nguonc/       Nguồn C API (JWPlayer + bootstrap/issue embed grant, HLS)
│   └── vi.fake-source/  SAMPLE source: fully fake data (runs offline, no network)
├── templates/           shared template crates for similar sites
└── public/              generated source list (gitignored, deployed by CI)
```

The CLI (`komorei repo build/verify/serve --root .`) discovers only this nested
layout: `<root>/sources/<id>/`.

> A crate containing a `.skip` file (like `vi.fake-source`) is excluded from
> `repo build` / `repo verify` / `repo serve` and from CI lint.

Structure of a single source:

```
sources/<id>/
├── Cargo.toml            # crate-type = ["cdylib"]; deps: komorei (+ serde for JSON)
├── .cargo/config.toml    # default build target = wasm32-unknown-unknown
├── res/
│   ├── source.json       # manifest (info.id/name/version/url/languages/contentRating)
│   └── icon.png          # source icon (128x128, opaque)
└── src/lib.rs            # komorei-sdk trait implementations + register_source!
```

The `source.json` ships in the package with the `Payload/{main.wasm,
source.json, icon.png}` layout (read by `KrxManager.readInfo` /
`extractMainWasm` in the app).

## Development

Requires the `komorei` CLI (komorei-sdk) and the wasm target:

```sh
cargo install --path ../komorei-sdk/crates/cli   # or wherever the SDK lives
rustup target add wasm32-unknown-unknown
```

Scaffold a new source and build the whole repo:

```sh
komorei init ophim --name "Example" --url https://example.com \
  --languages vi --content-rating safe
komorei repo build    # package every source and generate public/ (incl. index.min.json)
komorei repo serve    # build + serve locally (add the printed URL to the app to test)
komorei repo verify   # check every source package is valid before publishing
```

`init` auto-detects `komorei-sdk/` among the parent directories and wires the
dependency via a relative path (no need for the SDK to be published). Building
the whole repo means: compile each source to wasm → assemble the `.krx` with the
`Payload/` layout → collect everything into `public/sources/<id>-v<N>.krx` +
`public/icons/<id>-v<N>.png` + the `index.json`/`index.min.json` manifests.

Push to GitHub: the `.github/workflows/build.yaml` workflow compiles every
source and publishes `public/` to the `gh-pages` branch.

> Note: `package.krx` and `target/` are gitignored — they are reproducible
> artifacts.

## Installing sources into the app

- **Bundled**: copy `package.krx` into `app/src/main/assets/sources/` of the
  app repo — loaded by `KrxSourceRegistry` at startup (cannot be uninstalled).
- **User-installed**: `.krx` fetched from a repository via the Sources tab →
  `filesDir/sources/`.
- **JVM tests**: add a system property `komorei.test.<name>` in
  `app/build.gradle.kts` (see `komorei.test.ophimKrx`) and write a test like
  `OphimSourceRunnerIntegrationTest` — runs the real WASM through the runner
  cdylib + a real `KrxHostImpl`.

## Checklist for a new source

1. Use an id matching the `[A-Za-z0-9.\-]+` key pattern (required by
   `installKrx`).
2. Implement the `Source` trait (search/animeUpdate/streams) first, then layer
   the remaining traits (Listing/Home/Filters/Settings/DeepLink/Notification/
   Migration).
3. Build + verify: `komorei repo verify --root .`.
4. Write a JVM integration test against a local fixture (no dependency on a
   live domain — see `OphimSourceRunnerIntegrationTest`).
5. Update `res/source.json` and the source README.

See [CONTRIBUTING.md](CONTRIBUTING.md) for commit conventions and code style.