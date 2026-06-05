# Releasing code-ui

This project ships **prebuilt binaries** on GitHub Releases. You do not commit compiled files to git.

## Version numbers

The version lives in `Cargo.toml`:

```toml
[package]
version = "0.1.0"
```

Use [semantic versioning](https://semver.org/):

- **0.1.0 → 0.1.1** — bug fixes, small improvements
- **0.1.0 → 0.2.0** — new features, no breaking changes
- **0.1.0 → 1.0.0** — stable API or breaking changes

The git tag must match with a `v` prefix: `v0.1.0` for Cargo version `0.1.0`.

## Ship a release (maintainers)

1. Update `version` in `Cargo.toml` if needed.
2. Commit on your release branch and merge to `main` when ready.
3. Create and push a tag:

   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```

4. GitHub Actions (`.github/workflows/release.yml`) builds four binaries:
   - Linux x86_64 and arm64
   - macOS Intel and Apple Silicon
5. Open the new release on GitHub and confirm assets uploaded.

## How users install

**Recommended** — install script (needs a published release):

```bash
curl -fsSL https://raw.githubusercontent.com/motionharvest/code-ui/main/install.sh | bash
```

Pin a version:

```bash
CODE_UI_VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/motionharvest/code-ui/main/install.sh | bash
```

**Manual** — download the tarball for your platform from the [Releases](https://github.com/motionharvest/code-ui/releases) page, extract `code-ui`, and put it on your `PATH`.

**From source** (developers):

```bash
git clone git@github.com:motionharvest/code-ui.git
cd code-ui
# Requires Rust and Zig (see README)
cargo install --path .
```

## First release checklist

- [ ] `Cargo.toml` version set
- [ ] README install section accurate
- [ ] Tag pushed: `git push origin v0.1.0`
- [ ] GitHub Actions workflow succeeded
- [ ] Test install script on Linux and/or macOS
- [ ] Users have `tmux` and `git` installed
