# Templates

Template crates shared by multiple sources with similar backends (WordPress
themes, etc.). Each template provides a `Params` struct for per-source
configuration and an `Impl` trait containing the shared logic.

Create a new template:

```sh
komorei init templates/<name> --name "<Name>" --url https://example.com --languages multi --content-rating safe --template --template-name <name>
```

Create a source that uses a template:

```sh
komorei init sources/vi.<id> --name "<Name>" --url https://... --languages vi --template --template-name <name>
```

See [CONTRIBUTING.md](../CONTRIBUTING.md) for when a template should be created.