# Packager

Packager is a cross-platform Rust CLI for creating GitHub Release assets from a binary and source tree. It supports Windows, Linux, and macOS.

## Requirements

- Rust 1.88 or newer

## Build

```sh
cargo build --release
```

## Configuration file

Save the following as `PackagerConfig.toml` and run `packager --config PackagerConfig.toml`:

```toml
[project]
name = "smart-disk-cleaner"
version = "v1"
description = "An intelligent disk cleanup utility"

[paths]
binary = "target/release/smart-disk-cleaner.exe"
source = "."
output = "releases"

[target]
os = "windows"
arch = "x86_64"

[compression]
gzip = true
bzip2 = true
xz = true
zstd = true
zip = true
level = 9

[checksum]
enabled = true
algorithm = "sha256"
manifest = true

[packaging]
universal_source = true
include_binary = true
include_source = true
clean = false
```

`clean` must have a boolean value. Set it to `true` to remove existing output-directory contents before packaging. The default output directory is `releases`.

Any configuration value that has a CLI option can be overridden without changing the TOML file. For example:

```sh
packager --config PackagerConfig.toml --version v1.2.0 --output out --gzip false --checksum-algorithm sha512
```

## Direct CLI mode

Without `--config`, all six project and target arguments are required: `--name`, `--version`, `--os`, `--arch`, `--binary`, and `--source`. `--output` is optional and defaults to `releases`.

```sh
packager \
  --name smart-disk-cleaner \
  --version v1.0.0 \
  --os linux \
  --arch x86_64 \
  --binary ./target/release/smart-disk-cleaner \
  --source . \
  --output releases \
  --verify
```

The same invocation works on Windows after providing the platform's binary path and OS label.

## CLI options

| Option | Description |
|---|---|
| `--config FILE` | Load settings from a TOML file |
| `-n, --name NAME` | Project name |
| `-v, --version VERSION` | Release version |
| `--description TEXT` | Project description in the manifest |
| `-b, --binary FILE` | Binary file to package |
| `-s, --source DIR` | Source directory to package |
| `--os OS` | Target operating system label |
| `-a, --arch ARCH` | Target architecture label |
| `-o, --output DIR` | Output directory (default: `releases`) |
| `--compression-level LEVEL` | Compression level from 1 to 9 (default: 9) |
| `--gzip BOOL`, `--bzip2 BOOL`, `--xz BOOL`, `--zstd BOOL`, `--zip BOOL` | Enable or disable a format, using `true` or `false` |
| `--checksums BOOL` | Enable or disable checksum output |
| `--checksum-algorithm ALGORITHM` | `sha256` (default) or `sha512` |
| `--manifest BOOL` | Enable or disable the JSON release manifest |
| `--include-binary BOOL` | Enable or disable binary packages |
| `--include-source BOOL` | Enable or disable source packages |
| `--universal-source BOOL` | Keep source archive names platform-independent |
| `--clean BOOL` | Remove existing output contents before packaging |
| `--exclude-dir DIR` | Exclude a source directory by name; repeatable |
| `--verify` | Verify the generated checksum file |

## Generated assets

Binary packages include a raw binary copy and a TAR archive, plus enabled standalone compression formats and enabled compressed TAR or ZIP formats. Source packages include a TAR archive and enabled compressed TAR or ZIP formats. With `universal_source = true`, source filenames omit OS and architecture labels.

The tool writes a `<project>-<version>-release-manifest.json` file and a checksum file (`SHA256SUMS.txt` or `SHA512SUMS.txt`) when enabled. Checksums cover only artifacts generated during the current run, not unrelated files in the output directory. Source archive symlinks are skipped. The output directory is pruned during source traversal, including when it is nested inside the source tree.

Built-in source exclusions include `.git`, `.hg`, `.svn`, `.venv`, `venv`, `env`, `target`, `node_modules`, `__pycache__`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache`, `.tox`, `.next`, `.nuxt`, `.turbo`, `.cache`, `build`, `dist`, `coverage`, `packages`, `releases`, `.DS_Store`, `Thumbs.db`, and `*.pyc`. Additional directory names can be configured in `[packaging].exclude_dirs` or passed with `--exclude-dir`.
