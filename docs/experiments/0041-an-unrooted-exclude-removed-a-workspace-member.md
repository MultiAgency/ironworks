# An unrooted exclude removed a workspace member

The first installation of the Rust workspace on `multi-serve`, on 2026-09-02, copied the source
with this exclusion among the `rsync` rules:

```text
--exclude site/
```

The intent was to omit the generated top-level `site/` directory. The pattern was not anchored,
so it also matched `crates/site/`. The destination therefore held a `Cargo.toml` whose workspace
still named `crates/site`, but not the member itself. Cargo refused to load the workspace.

## What was measured

- Source: the primary workstation's current RustWorks tree.
- Destination: `/opt/rustworks` on the Ubuntu 24.04 `multi-serve` host.
- Build environment: `rust:1.97-bookworm`, with the destination mounted at `/src`.
- Result with the unrooted rule: workspace loading failed because `crates/site` was absent.
- Result after replacing it with `--exclude /site/`: the member was present and the release build
  completed.

The other top-level exclusions are rooted for the same reason:

```text
--exclude /target/ --exclude /.git/ --exclude /site/ --exclude /local/
```

## Finding

An `rsync` exclusion without a leading slash is not a top-level exclusion. In this workspace it
can remove a Cargo member while leaving the manifest that names it, producing an incomplete source
tree rather than a smaller valid one.

This is a RustWorks deployment-procedure defect, not an IronClaw defect. The runtime pin is not
causal; it is recorded because this was found while installing the deployment that uses that pin.

## What changed

`deploy/README.md` now roots the generated-directory exclusions. The host was resynchronised and
the release binaries were rebuilt from the complete workspace rather than accepted from an older
build cache.

