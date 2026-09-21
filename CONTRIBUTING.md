# Contributing

Thank you for your interest in contributing to this source collection!

All changes go through pull requests that are squashed on merge, using
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/). Examples:

- `feat: add vi.ophim`
- `fix(vi.ophim): update base url`
- `feat(vi.ophim): add filtering and home listings`
- `chore: remove en.example`

Unrelated changes should go into separate PRs (a PR usually does not add two
independent sources at once).

## New source checklist

Before submitting, make sure:

- [ ] The source compiles without any warnings or errors
- [ ] `cargo fmt` has been run before submission
- [ ] `cargo clippy` outputs no lint warnings (`cargo clippy -- -D warnings`)
- [ ] All files end with an additional newline
- [ ] JSON files use tabs for indentation
- [ ] `komorei verify <path to package.krx>` passes

## Minimum source functionality

Sources should generally (when the site provides the corresponding data):

- Set `contentRating` in `res/source.json`
- Implement `ListingProvider`/home listings if the site supports it
- Implement `DeepLinkHandler` for series/episode URLs, if possible
- Ship filter options matching the site's filters

Implementing home pages and listings is encouraged but not required for every
source. The `vi.ophim` and `vi.fake-source` sources in this repo are good
references.

## Templates

Sites sharing a common backend (WordPress themes, etc.) should reuse a
[template crate](templates/) instead of duplicating parser code. Templates
provide a `Params` struct for per-source configuration and an `Impl` trait with
the shared logic. See the `templates/` directory in
`Aidoku-Community/sources` for prior art.

## General tips

Avoid unnecessary `clone`s: sources run in a constrained wasm environment, so
function parameters should take references (`&str` instead of `String`) and
iteration should use `iter()` instead of `into_iter()`. A `RefCell` may be used
to keep mutable state on a source struct (the environment is single-threaded —
no locks needed).