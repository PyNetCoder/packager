use anyhow::{Context, Result, bail};
use bzip2::Compression as BzCompression;
use bzip2::write::BzEncoder;
use clap::{ArgAction, Parser};
use flate2::Compression as GzCompression;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use tar::Builder as TarBuilder;
use walkdir::{DirEntry, WalkDir};
use xz2::write::XzEncoder;
use zip::write::{SimpleFileOptions, ZipWriter};

#[derive(Parser, Debug)]
#[command(
    name = "packager",
    version,
    about = "GitHub Release packager with configurable archives and checksums",
    long_about = "Creates binary packages and source archives for GitHub Releases.",
    disable_version_flag = true
)]
struct Args {
    /// TOML configuration file. When omitted, the six direct-mode arguments are required.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Generate a default PackagerConfig.toml file in the current directory
    #[arg(long)]
    init: bool,

    /// Print version information
    #[arg(short = 'V', action = ArgAction::Version)]
    cli_version: Option<bool>,

    /// Project name, for example: smart-disk-cleaner
    #[arg(short, long, value_name = "NAME")]
    name: Option<String>,

    /// Release version, for example: v1.0.0
    #[arg(short, long, value_name = "VERSION")]
    version: Option<String>,

    /// Project description for the release manifest
    #[arg(long, value_name = "TEXT")]
    description: Option<String>,

    /// Executable or cache file to package
    #[arg(short, long, value_name = "FILE")]
    binary: Option<PathBuf>,

    /// Source directory to package
    #[arg(short, long, value_name = "DIR")]
    source: Option<PathBuf>,

    /// Target operating system, for example: windows, linux, macos
    #[arg(long, value_name = "OS")]
    os: Option<String>,

    /// Target architecture, for example: x86_64, aarch64, x86
    #[arg(short = 'a', long, value_name = "ARCH")]
    arch: Option<String>,

    /// Output directory
    #[arg(short, long, value_name = "DIR")]
    output: Option<PathBuf>,

    /// Compression level (1-9). Higher means smaller files and usually slower packaging.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=9))]
    compression_level: Option<u32>,

    /// Exclude an additional directory name from source archives. Can be repeated.
    #[arg(long = "exclude-dir", value_name = "DIR", action = ArgAction::Append)]
    exclude_dirs: Vec<String>,

    /// Override a compression setting from the configuration (true or false).
    #[arg(long, value_name = "BOOL", value_parser = clap::value_parser!(bool))]
    gzip: Option<bool>,

    /// Override a compression setting from the configuration (true or false).
    #[arg(long, value_name = "BOOL", value_parser = clap::value_parser!(bool))]
    bzip2: Option<bool>,

    /// Override a compression setting from the configuration (true or false).
    #[arg(long, value_name = "BOOL", value_parser = clap::value_parser!(bool))]
    xz: Option<bool>,

    /// Override a compression setting from the configuration (true or false).
    #[arg(long, value_name = "BOOL", value_parser = clap::value_parser!(bool))]
    zstd: Option<bool>,

    /// Override a compression setting from the configuration (true or false).
    #[arg(long, value_name = "BOOL", value_parser = clap::value_parser!(bool))]
    zip: Option<bool>,

    /// Override whether checksums are generated (true or false).
    #[arg(long, value_name = "BOOL", value_parser = clap::value_parser!(bool))]
    checksums: Option<bool>,

    /// Checksum algorithm: sha256 or sha512.
    #[arg(long, value_name = "ALGORITHM")]
    checksum_algorithm: Option<String>,

    /// Override whether the JSON release manifest is generated (true or false).
    #[arg(long, value_name = "BOOL", value_parser = clap::value_parser!(bool))]
    manifest: Option<bool>,

    /// Override binary packaging (true or false).
    #[arg(long, value_name = "BOOL", value_parser = clap::value_parser!(bool))]
    include_binary: Option<bool>,

    /// Override source packaging (true or false).
    #[arg(long, value_name = "BOOL", value_parser = clap::value_parser!(bool))]
    include_source: Option<bool>,

    /// Override platform-independent source naming (true or false).
    #[arg(long, value_name = "BOOL", value_parser = clap::value_parser!(bool))]
    universal_source: Option<bool>,

    /// Remove all existing contents of the output directory before packaging.
    #[arg(long, value_name = "BOOL", value_parser = clap::value_parser!(bool))]
    clean: Option<bool>,

    /// Verify every entry in the generated checksum file after packaging
    #[arg(long, default_value_t = false)]
    verify: bool,
}

#[derive(Serialize)]
struct ArtifactInfo {
    file: String,
    size_bytes: u64,
    checksum: String,
}

#[derive(Serialize)]
struct ReleaseManifest {
    schema_version: u32,
    tool: &'static str,
    project: String,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_os: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_arch: Option<String>,
    compression_level: u32,
    checksum_algorithm: String,
    source_file_count: usize,
    artifacts: Vec<ArtifactInfo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    project: ProjectConfig,
    paths: PathsConfig,
    target: TargetConfig,
    compression: CompressionConfig,
    checksum: ChecksumConfig,
    packaging: PackagingConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ProjectConfig {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PathsConfig {
    binary: Option<PathBuf>,
    source: Option<PathBuf>,
    output: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TargetConfig {
    os: Option<String>,
    arch: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CompressionConfig {
    gzip: bool,
    bzip2: bool,
    xz: bool,
    zstd: bool,
    zip: bool,
    level: u32,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            gzip: true,
            bzip2: true,
            xz: true,
            zstd: true,
            zip: true,
            level: 9,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ChecksumConfig {
    enabled: bool,
    algorithm: String,
    manifest: bool,
}

impl Default for ChecksumConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            algorithm: "sha256".to_owned(),
            manifest: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PackagingConfig {
    universal_source: bool,
    include_binary: bool,
    include_source: bool,
    clean: bool,
    exclude_dirs: Vec<String>,
}

impl Default for PackagingConfig {
    fn default() -> Self {
        Self {
            universal_source: true,
            include_binary: true,
            include_source: true,
            clean: false,
            exclude_dirs: Vec::new(),
        }
    }
}

struct Settings {
    name: String,
    version: String,
    description: Option<String>,
    binary: Option<PathBuf>,
    source: Option<PathBuf>,
    output: PathBuf,
    os: Option<String>,
    arch: Option<String>,
    compression: CompressionConfig,
    checksum: ChecksumConfig,
    packaging: PackagingConfig,
    verify: bool,
}

#[derive(Clone, Copy)]
enum ChecksumAlgorithm {
    Sha256,
    Sha512,
}

impl ChecksumAlgorithm {
    fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().replace('-', "").as_str() {
            "sha256" => Ok(Self::Sha256),
            "sha512" => Ok(Self::Sha512),
            _ => bail!(
                "Unsupported checksum algorithm '{value}'. Supported values are sha256 and sha512."
            ),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("\n❌ Error: {error:#}");
        std::process::exit(1);
    }
}

const DEFAULT_CONFIG_TEMPLATE: &str = r#"[project]
name = "name"
version = "v1"
description = "description"

[paths]
binary = "./target/release/binary"
source = "."
output = "releases"

[target]
os = "os"
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
"#;

fn init_config() -> Result<()> {
    let config_path = Path::new("PackagerConfig.toml");
    if config_path.exists() {
        bail!("PackagerConfig.toml already exists.");
    }
    fs::write(config_path, DEFAULT_CONFIG_TEMPLATE)
        .with_context(|| "Could not write PackagerConfig.toml")?;
    println!("Created PackagerConfig.toml");
    Ok(())
}

fn run() -> Result<()> {
    let args = Args::parse();

    if args.init {
        return init_config();
    }

    let config = if let Some(config_path) = &args.config {
        let content = fs::read_to_string(config_path).with_context(|| {
            format!(
                "Could not read configuration file: {}",
                config_path.display()
            )
        })?;
        toml::from_str::<Config>(&content).with_context(|| {
            format!(
                "Could not parse TOML configuration: {}",
                config_path.display()
            )
        })?
    } else {
        require_direct_arguments(&args)?;
        Config::default()
    };
    let settings = resolve_settings(args, config)?;
    run_packaging(settings)
}

fn require_direct_arguments(args: &Args) -> Result<()> {
    let required = [
        ("--name", args.name.is_some()),
        ("--version", args.version.is_some()),
        ("--os", args.os.is_some()),
        ("--arch", args.arch.is_some()),
        ("--binary", args.binary.is_some()),
        ("--source", args.source.is_some()),
    ];
    let missing = required
        .iter()
        .filter_map(|(flag, present)| (!present).then_some(*flag));
    let missing = missing.collect::<Vec<_>>();
    if !missing.is_empty() {
        bail!(
            "Direct mode requires {}. Use --config to load settings from a TOML file.",
            missing.join(", ")
        );
    }
    Ok(())
}

fn resolve_settings(args: Args, config: Config) -> Result<Settings> {
    let name = args
        .name
        .or(config.project.name)
        .context("Project name is required (--name or [project].name).");
    let version = args
        .version
        .or(config.project.version)
        .context("Project version is required (--version or [project].version).");
    let binary = args.binary.or(config.paths.binary);
    let source = args.source.or(config.paths.source);
    let name = name?;
    let version = version?;
    let output = args
        .output
        .or(config.paths.output)
        .unwrap_or_else(|| PathBuf::from("releases"));

    let mut compression = config.compression;
    compression.level = args.compression_level.unwrap_or(compression.level);
    compression.gzip = args.gzip.unwrap_or(compression.gzip);
    compression.bzip2 = args.bzip2.unwrap_or(compression.bzip2);
    compression.xz = args.xz.unwrap_or(compression.xz);
    compression.zstd = args.zstd.unwrap_or(compression.zstd);
    compression.zip = args.zip.unwrap_or(compression.zip);

    let mut checksum = config.checksum;
    checksum.enabled = args.checksums.unwrap_or(checksum.enabled);
    checksum.manifest = args.manifest.unwrap_or(checksum.manifest);
    checksum.algorithm = args.checksum_algorithm.unwrap_or(checksum.algorithm);

    let mut packaging = config.packaging;
    packaging.include_binary = args.include_binary.unwrap_or(packaging.include_binary);
    packaging.include_source = args.include_source.unwrap_or(packaging.include_source);
    packaging.universal_source = args.universal_source.unwrap_or(packaging.universal_source);
    packaging.clean = args.clean.unwrap_or(packaging.clean);
    if !args.exclude_dirs.is_empty() {
        packaging.exclude_dirs = args.exclude_dirs;
    }

    Ok(Settings {
        name,
        version,
        description: args.description.or(config.project.description),
        binary,
        source,
        output,
        os: args.os.or(config.target.os),
        arch: args.arch.or(config.target.arch),
        compression,
        checksum,
        packaging,
        verify: args.verify,
    })
}

fn run_packaging(settings: Settings) -> Result<()> {
    validate_component("project name", &settings.name)?;
    validate_component("version", &settings.version)?;
    if let Some(os) = &settings.os {
        validate_component("operating system", os)?;
    }
    if let Some(arch) = &settings.arch {
        validate_component("architecture", arch)?;
    }
    for directory in &settings.packaging.exclude_dirs {
        validate_component("excluded directory name", directory)?;
    }
    if !(1..=9).contains(&settings.compression.level) {
        bail!("Compression level must be between 1 and 9.");
    }
    if !settings.packaging.include_binary && !settings.packaging.include_source {
        bail!("At least one of include_binary or include_source must be enabled.");
    }
    if settings.verify && !settings.checksum.enabled {
        bail!("--verify requires checksum generation to be enabled.");
    }
    let algorithm = ChecksumAlgorithm::parse(&settings.checksum.algorithm)?;

    let binary_path = if settings.packaging.include_binary || settings.packaging.clean {
        settings
            .binary
            .as_ref()
            .map(|path| {
                let resolved = fs::canonicalize(path)
                    .with_context(|| format!("Could not access binary: {}", path.display()))?;
                if !resolved.is_file() {
                    bail!("Binary path is not a regular file: {}", resolved.display());
                }
                Ok::<_, anyhow::Error>(resolved)
            })
            .transpose()?
    } else {
        None
    };
    if settings.packaging.include_binary && binary_path.is_none() {
        bail!("Binary path is required when binary packaging is enabled.");
    }

    let source_path = if settings.packaging.include_source || settings.packaging.clean {
        settings
            .source
            .as_ref()
            .map(|path| {
                let resolved = fs::canonicalize(path).with_context(|| {
                    format!("Could not access source directory: {}", path.display())
                })?;
                if !resolved.is_dir() {
                    bail!("Source path is not a directory: {}", resolved.display());
                }
                Ok::<_, anyhow::Error>(resolved)
            })
            .transpose()?
    } else {
        None
    };
    if settings.packaging.include_source && source_path.is_none() {
        bail!("Source path is required when source packaging is enabled.");
    }

    fs::create_dir_all(&settings.output).with_context(|| {
        format!(
            "Could not create output directory: {}",
            settings.output.display()
        )
    })?;
    let output_path = fs::canonicalize(&settings.output).with_context(|| {
        format!(
            "Could not resolve output directory: {}",
            settings.output.display()
        )
    })?;
    if let Some(source) = &source_path {
        if output_path == *source {
            bail!("The output directory cannot be the source directory itself.");
        }
        if settings.packaging.clean && source.starts_with(&output_path) {
            bail!(
                "The output directory cannot contain the source directory when clean is enabled."
            );
        }
    }
    if settings.packaging.clean {
        let current_dir =
            fs::canonicalize(".").context("Could not resolve the current directory")?;
        if current_dir.starts_with(&output_path) || output_path.parent().is_none() {
            bail!("The output directory is too broad to clean safely.");
        }
        if let Some(binary) = &binary_path
            && binary.starts_with(&output_path)
        {
            bail!("The output directory cannot contain the input binary when clean is enabled.");
        }
        clear_directory(&output_path)?;
    }

    let mut binary_parts = vec![settings.name.clone(), settings.version.clone()];
    if let Some(os) = &settings.os {
        binary_parts.push(os.clone());
    }
    if let Some(arch) = &settings.arch {
        binary_parts.push(arch.clone());
    }
    let binary_prefix = format!("{}-binary", binary_parts.join("-"));
    let source_prefix = if settings.packaging.universal_source {
        format!("{}-{}-source", settings.name, settings.version)
    } else {
        let mut parts = vec![settings.name.clone(), settings.version.clone()];
        if let Some(os) = &settings.os {
            parts.push(os.clone());
        }
        if let Some(arch) = &settings.arch {
            parts.push(arch.clone());
        }
        format!("{}-source", parts.join("-"))
    };

    println!("Packaging '{}' ({})", settings.name, settings.version);
    println!("Output: {}", output_path.display());
    println!("Compression level: {}", settings.compression.level);

    let mut artifacts = Vec::<PathBuf>::new();
    let mut source_file_count = 0usize;
    if let Some(binary) = &binary_path {
        println!("\nCreating binary packages...");
        let binary_filename = binary
            .file_name()
            .context("Binary path has no filename")?
            .to_string_lossy();
        let raw_binary_path = output_path.join(format!("{binary_prefix}-{binary_filename}"));
        if binary == &raw_binary_path {
            bail!("The input binary and output binary path are identical.");
        }
        fs::copy(binary, &raw_binary_path)
            .with_context(|| format!("Could not copy binary to {}", raw_binary_path.display()))?;
        artifacts.push(raw_binary_path);
        pack_single_file(
            binary,
            &output_path,
            &binary_prefix,
            &settings.compression,
            &mut artifacts,
        )?;
    }
    if let Some(source) = &source_path {
        println!("\nCreating source packages...");
        source_file_count = pack_source_directory(
            source,
            &output_path,
            &source_prefix,
            &settings.compression,
            &settings.packaging.exclude_dirs,
            &output_path,
            &mut artifacts,
        )?;
    }

    if settings.checksum.manifest {
        println!("\nCreating release manifest...");
        let manifest_path = write_manifest(
            &output_path,
            &settings,
            source_file_count,
            &artifacts,
            algorithm,
        )?;
        artifacts.push(manifest_path);
    }

    let checksum_path = if settings.checksum.enabled {
        let checksum_name = format!("{}SUMS.txt", algorithm.name().to_ascii_uppercase());
        println!("\nCreating {checksum_name}...");
        Some(write_checksums(&output_path, &artifacts, algorithm)?)
    } else {
        None
    };

    if settings.verify {
        let checksum_path = checksum_path
            .as_ref()
            .context("Checksum file was not generated.")?;
        println!("\nVerifying checksums...");
        verify_checksums(checksum_path, &output_path, algorithm)?;
        println!("Checksum verification passed.");
    }

    println!("\nPackaging completed successfully.");
    if source_path.is_some() {
        println!("Source files included: {source_file_count}");
    }
    let release_file_count = artifacts.len() + if checksum_path.is_some() { 1 } else { 0 };
    println!("Release files created: {release_file_count}");
    if let Some(checksum_path) = &checksum_path {
        println!("Checksum file: {}", checksum_path.display());
    }
    println!("\nGenerated files:");
    for path in artifacts.iter().chain(checksum_path.iter()) {
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        println!("  {name} ({})", format_size(size));
    }
    println!("\nDone. Check: {}", output_path.display());
    Ok(())
}

fn clear_directory(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)
        .with_context(|| format!("Could not read output directory: {}", path.display()))?
    {
        let entry = entry?;
        let entry_path = entry.path();
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(&entry_path)
                .with_context(|| format!("Could not remove {}", entry_path.display()))?;
        } else {
            fs::remove_file(&entry_path)
                .with_context(|| format!("Could not remove {}", entry_path.display()))?;
        }
    }
    Ok(())
}

fn validate_component(label: &str, value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value != "."
        && value != ".."
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        });

    if !valid {
        bail!(
            "Invalid {label}: '{value}'. Use only ASCII letters, numbers, dots, underscores and hyphens."
        );
    }
    Ok(())
}

fn pack_single_file(
    file_path: &Path,
    out_dir: &Path,
    prefix: &str,
    compression: &CompressionConfig,
    artifacts: &mut Vec<PathBuf>,
) -> Result<()> {
    let file_name = file_path
        .file_name()
        .context("Binary path has no filename")?
        .to_string_lossy()
        .into_owned();

    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();

    if compression.gzip {
        let path = out_dir.join(format!("{prefix}{ext}.gz"));
        compress_gzip(file_path, &path, compression.level)?;
        artifacts.push(path);
    }
    if compression.bzip2 {
        let path = out_dir.join(format!("{prefix}{ext}.bz2"));
        compress_bzip2(file_path, &path, compression.level)?;
        artifacts.push(path);
    }
    if compression.xz {
        let path = out_dir.join(format!("{prefix}{ext}.xz"));
        compress_xz(file_path, &path, compression.level)?;
        artifacts.push(path);
    }
    if compression.zstd {
        let path = out_dir.join(format!("{prefix}{ext}.zst"));
        compress_zstd(file_path, &path, compression.level)?;
        artifacts.push(path);
    }

    let tar_path = out_dir.join(format!("{prefix}.tar"));
    let tar_writer = write_single_tar(File::create(&tar_path)?, file_path)?;
    drop(tar_writer);
    artifacts.push(tar_path.clone());

    if compression.gzip {
        let path = out_dir.join(format!("{prefix}.tar.gz"));
        compress_gzip(&tar_path, &path, compression.level)?;
        artifacts.push(path);
    }
    if compression.bzip2 {
        let path = out_dir.join(format!("{prefix}.tar.bz2"));
        compress_bzip2(&tar_path, &path, compression.level)?;
        artifacts.push(path);
    }
    if compression.xz {
        let path = out_dir.join(format!("{prefix}.tar.xz"));
        compress_xz(&tar_path, &path, compression.level)?;
        artifacts.push(path);
    }
    if compression.zstd {
        let path = out_dir.join(format!("{prefix}.tar.zst"));
        compress_zstd(&tar_path, &path, compression.level)?;
        artifacts.push(path);
    }

    if compression.zip {
        let zip_path = out_dir.join(format!("{prefix}{ext}.zip"));
        let mut zip = ZipWriter::new(File::create(&zip_path)?);
        zip.start_file(file_name, zip_options(compression.level))?;
        let mut input = BufReader::new(File::open(file_path)?);
        io::copy(&mut input, &mut zip)?;
        zip.finish()?;
        artifacts.push(zip_path);
    }

    println!("   ✅ Binary package set created: {prefix}");
    Ok(())
}

fn write_single_tar<W: Write>(writer: W, file_path: &Path) -> Result<W> {
    let file_name = file_path
        .file_name()
        .context("Binary path has no filename")?;
    let mut archive = TarBuilder::new(writer);
    archive.follow_symlinks(false);
    archive.append_path_with_name(file_path, Path::new(file_name))?;
    archive.finish()?;
    Ok(archive.into_inner()?)
}

fn pack_source_directory(
    source_dir: &Path,
    out_dir: &Path,
    prefix: &str,
    compression: &CompressionConfig,
    exclude_dirs: &[String],
    output_dir: &Path,
    artifacts: &mut Vec<PathBuf>,
) -> Result<usize> {
    let tar_path = out_dir.join(format!("{prefix}.tar"));
    let (tar_writer, file_count) = create_source_tar(
        File::create(&tar_path)?,
        source_dir,
        exclude_dirs,
        output_dir,
    )?;
    drop(tar_writer);
    artifacts.push(tar_path.clone());
    if compression.gzip {
        let path = out_dir.join(format!("{prefix}.tar.gz"));
        compress_gzip(&tar_path, &path, compression.level)?;
        artifacts.push(path);
    }
    if compression.bzip2 {
        let path = out_dir.join(format!("{prefix}.tar.bz2"));
        compress_bzip2(&tar_path, &path, compression.level)?;
        artifacts.push(path);
    }
    if compression.xz {
        let path = out_dir.join(format!("{prefix}.tar.xz"));
        compress_xz(&tar_path, &path, compression.level)?;
        artifacts.push(path);
    }
    if compression.zstd {
        let path = out_dir.join(format!("{prefix}.tar.zst"));
        compress_zstd(&tar_path, &path, compression.level)?;
        artifacts.push(path);
    }
    if compression.zip {
        let zip_path = out_dir.join(format!("{prefix}.zip"));
        let zip_file_count = create_source_zip(
            source_dir,
            &zip_path,
            exclude_dirs,
            output_dir,
            compression.level,
        )?;
        artifacts.push(zip_path);
        if zip_file_count != file_count {
            eprintln!(
                "Archive entry count differs (TAR: {file_count}, ZIP: {zip_file_count}). Check for filesystem-specific paths."
            );
        }
    }

    println!("   ✅ Source package set created: {prefix}");
    Ok(file_count)
}

fn create_source_tar<W: Write>(
    writer: W,
    source_dir: &Path,
    exclude_dirs: &[String],
    output_dir: &Path,
) -> Result<(W, usize)> {
    let mut archive = TarBuilder::new(writer);
    archive.follow_symlinks(false);
    let mut file_count = 0usize;

    for entry in WalkDir::new(source_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_ignored(entry, exclude_dirs, output_dir))
    {
        let entry = entry.with_context(|| "Failed while scanning source directory")?;
        if entry.depth() == 0 || !entry.file_type().is_file() {
            // Symlinks are intentionally skipped; they are never followed.
            continue;
        }

        let relative_path = entry
            .path()
            .strip_prefix(source_dir)
            .with_context(|| format!("Could not make path relative: {}", entry.path().display()))?;
        archive.append_path_with_name(entry.path(), relative_path)?;
        file_count += 1;
    }

    archive.finish()?;
    let writer = archive.into_inner()?;
    Ok((writer, file_count))
}

fn create_source_zip(
    source_dir: &Path,
    zip_path: &Path,
    exclude_dirs: &[String],
    output_dir: &Path,
    level: u32,
) -> Result<usize> {
    let mut zip = ZipWriter::new(File::create(zip_path)?);
    let mut file_count = 0usize;

    for entry in WalkDir::new(source_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_ignored(entry, exclude_dirs, output_dir))
    {
        let entry = entry.with_context(|| "Failed while scanning source directory for ZIP")?;
        if entry.depth() == 0 || !entry.file_type().is_file() {
            continue;
        }

        let relative_path = entry.path().strip_prefix(source_dir)?;
        let archive_name = relative_path_to_zip_name(relative_path)?;
        zip.start_file(archive_name, zip_options(level))?;

        let mut input = BufReader::new(File::open(entry.path())?);
        io::copy(&mut input, &mut zip)?;
        file_count += 1;
    }

    zip.finish()?;
    Ok(file_count)
}

fn relative_path_to_zip_name(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => bail!(
                "Unexpected non-relative path in source archive: {}",
                path.display()
            ),
        }
    }

    if parts.is_empty() {
        bail!("Cannot create an empty ZIP entry name.");
    }
    Ok(parts.join("/"))
}

fn is_ignored(entry: &DirEntry, custom_excludes: &[String], output_dir: &Path) -> bool {
    if entry.path() == output_dir {
        return true;
    }
    const IGNORED_DIRS: &[&str] = &[
        ".git",
        ".hg",
        ".svn",
        ".venv",
        "venv",
        "env",
        "target",
        "node_modules",
        "__pycache__",
        ".pytest_cache",
        ".mypy_cache",
        ".ruff_cache",
        ".tox",
        ".next",
        ".nuxt",
        ".turbo",
        ".cache",
        "build",
        "dist",
        "coverage",
        "packages",
        "releases",
    ];

    let name = entry.file_name().to_string_lossy();
    if entry.file_type().is_dir() {
        return IGNORED_DIRS.contains(&name.as_ref())
            || custom_excludes
                .iter()
                .any(|excluded| excluded == name.as_ref());
    }

    name == ".DS_Store" || name == "Thumbs.db" || name.ends_with(".pyc")
}

fn zip_options(level: u32) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(level as i64))
}

fn compress_gzip(input_path: &Path, output_path: &Path, level: u32) -> Result<()> {
    let mut input = BufReader::new(File::open(input_path)?);
    let mut encoder = GzEncoder::new(File::create(output_path)?, GzCompression::new(level));
    io::copy(&mut input, &mut encoder)?;
    encoder.finish()?;
    Ok(())
}

fn compress_bzip2(input_path: &Path, output_path: &Path, level: u32) -> Result<()> {
    let mut input = BufReader::new(File::open(input_path)?);
    let mut encoder = BzEncoder::new(File::create(output_path)?, BzCompression::new(level));
    io::copy(&mut input, &mut encoder)?;
    encoder.finish()?;
    Ok(())
}

fn compress_xz(input_path: &Path, output_path: &Path, level: u32) -> Result<()> {
    let mut input = BufReader::new(File::open(input_path)?);
    let mut encoder = XzEncoder::new(File::create(output_path)?, level);
    io::copy(&mut input, &mut encoder)?;
    encoder.finish()?;
    Ok(())
}

fn compress_zstd(input_path: &Path, output_path: &Path, level: u32) -> Result<()> {
    let mut input = BufReader::new(File::open(input_path)?);
    let mut encoder = zstd::stream::write::Encoder::new(File::create(output_path)?, level as i32)?;
    io::copy(&mut input, &mut encoder)?;
    encoder.finish()?;
    Ok(())
}

fn write_manifest(
    out_dir: &Path,
    settings: &Settings,
    source_file_count: usize,
    artifacts: &[PathBuf],
    algorithm: ChecksumAlgorithm,
) -> Result<PathBuf> {
    let mut artifact_infos = Vec::with_capacity(artifacts.len());

    for path in artifacts {
        let metadata = fs::metadata(path)
            .with_context(|| format!("Could not read artifact metadata: {}", path.display()))?;
        let file_name = path
            .file_name()
            .context("Artifact path has no filename")?
            .to_string_lossy()
            .into_owned();

        artifact_infos.push(ArtifactInfo {
            file: file_name,
            size_bytes: metadata.len(),
            checksum: checksum_file(path, algorithm)?,
        });
    }

    artifact_infos.sort_by(|a, b| a.file.cmp(&b.file));

    let manifest = ReleaseManifest {
        schema_version: 2,
        tool: "packager",
        project: settings.name.clone(),
        version: settings.version.clone(),
        description: settings.description.clone(),
        target_os: settings.os.clone(),
        target_arch: settings.arch.clone(),
        compression_level: settings.compression.level,
        checksum_algorithm: algorithm.name().to_owned(),
        source_file_count,
        artifacts: artifact_infos,
    };

    let manifest_path = out_dir.join(format!(
        "{}-{}-release-manifest.json",
        settings.name, settings.version
    ));
    let json =
        serde_json::to_vec_pretty(&manifest).context("Could not serialize release manifest")?;
    fs::write(&manifest_path, json)
        .with_context(|| format!("Could not write manifest: {}", manifest_path.display()))?;

    Ok(manifest_path)
}

fn write_checksums(
    out_dir: &Path,
    artifacts: &[PathBuf],
    algorithm: ChecksumAlgorithm,
) -> Result<PathBuf> {
    let checksum_path = out_dir.join(format!("{}SUMS.txt", algorithm.name().to_ascii_uppercase()));
    let mut sorted_artifacts = artifacts.to_vec();
    sorted_artifacts.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    let file = File::create(&checksum_path).with_context(|| {
        format!(
            "Could not create checksum file: {}",
            checksum_path.display()
        )
    })?;
    let mut writer = BufWriter::new(file);

    for path in sorted_artifacts {
        let digest = checksum_file(&path, algorithm)?;
        let file_name = path
            .file_name()
            .context("Artifact path has no filename")?
            .to_string_lossy();
        writeln!(writer, "{digest}  {file_name}")?;
    }

    writer.flush()?;
    Ok(checksum_path)
}

fn verify_checksums(
    checksum_path: &Path,
    out_dir: &Path,
    algorithm: ChecksumAlgorithm,
) -> Result<()> {
    let reader = BufReader::new(File::open(checksum_path)?);
    let mut verified = 0usize;

    for (line_number, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let (expected_hash, file_name) = line
            .split_once("  ")
            .with_context(|| format!("Invalid checksum line {}", line_number + 1))?;

        if file_name.contains('/') || file_name.contains('\\') {
            bail!("Unsafe path in checksum file: {file_name}");
        }

        let name_path = Path::new(file_name);
        if name_path.components().count() != 1 {
            bail!("Unsafe path in checksum file: {file_name}");
        }

        let artifact_path = out_dir.join(name_path);
        let actual_hash = checksum_file(&artifact_path, algorithm)
            .with_context(|| format!("Could not verify artifact: {file_name}"))?;

        if actual_hash != expected_hash {
            bail!("Checksum mismatch: {file_name}");
        }
        verified += 1;
    }

    if verified == 0 {
        bail!("Checksum file contains no entries.");
    }

    println!("   ✅ Verified {verified} file(s).");
    Ok(())
}

fn checksum_file(path: &Path, algorithm: ChecksumAlgorithm) -> Result<String> {
    let mut file = BufReader::new(
        File::open(path).with_context(|| format!("Could not open {}", path.display()))?,
    );
    let mut buffer = [0u8; 64 * 1024];
    match algorithm {
        ChecksumAlgorithm::Sha256 => {
            let mut hasher = Sha256::new();
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Ok(hex_digest(&hasher.finalize()))
        }
        ChecksumAlgorithm::Sha512 => {
            let mut hasher = Sha512::new();
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Ok(hex_digest(&hasher.finalize()))
        }
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let value = bytes as f64;

    if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.2} MB", value / MB)
    } else if value >= KB {
        format!("{:.2} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}
