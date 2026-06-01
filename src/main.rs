use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    process::ExitCode,
    thread,
    time::{Duration, SystemTime},
};

use clap::{Args, Parser, Subcommand};
use xluau::{
    compiler::Compiler,
    formatter::format_source,
    package_manager::{BundleOptions, PackageManager},
    source_map::remap_trace,
};

#[derive(Debug, Parser)]
#[command(name = "xluau")]
#[command(about = "XLuau compiler")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Build(BuildArgs),
    Check(CheckArgs),
    Fmt(FmtArgs),
    Run(RunArgs),
    Install(PackageArgs),
    Remove(PackageArgs),
    Update(PackageArgs),
    List(ProjectPathArgs),
    Bundle(BundleArgs),
    Publish(PublishArgs),
    Remap { stacktrace: PathBuf },
}

#[derive(Debug, Clone, Args)]
struct BuildArgs {
    path: Option<PathBuf>,
    #[arg(long)]
    watch: bool,
}

#[derive(Debug, Clone, Args)]
struct CheckArgs {
    path: Option<PathBuf>,
    #[arg(long)]
    watch: bool,
}

#[derive(Debug, Clone, Args)]
struct FmtArgs {
    path: Option<PathBuf>,
    #[arg(long)]
    check: bool,
}

#[derive(Debug, Clone, Args)]
struct RunArgs {
    path: PathBuf,
    #[arg(long)]
    runtime: Option<String>,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug, Clone, Args)]
struct PackageArgs {
    #[arg()]
    packages: Vec<String>,
    #[arg(long)]
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
struct ProjectPathArgs {
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
struct BundleArgs {
    path: Option<PathBuf>,
    #[arg(long)]
    no_minify: bool,
}

#[derive(Debug, Clone, Args)]
struct PublishArgs {
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
enum Operation {
    Build,
    Check,
}

#[derive(Debug, Clone)]
enum InvocationTarget {
    ProjectRoot(PathBuf),
    SingleFile {
        compiler_root: PathBuf,
        file_path: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    match cli.command {
        Command::Build(args) => run_operation(Operation::Build, args.path, args.watch, &cwd)?,
        Command::Check(args) => run_operation(Operation::Check, args.path, args.watch, &cwd)?,
        Command::Fmt(args) => run_format(args.path, args.check, &cwd)?,
        Command::Run(args) => run_file(args, &cwd)?,
        Command::Install(args) => run_install(args, &cwd)?,
        Command::Remove(args) => run_remove(args, &cwd)?,
        Command::Update(args) => run_update(args, &cwd)?,
        Command::List(args) => run_list(args, &cwd)?,
        Command::Bundle(args) => run_bundle(args, &cwd)?,
        Command::Publish(args) => run_publish(args, &cwd)?,
        Command::Remap { stacktrace } => {
            let trace = fs::read_to_string(&stacktrace)?;
            println!("{}", remap_trace(&trace, &cwd));
        }
    }
    Ok(())
}

fn run_install(args: PackageArgs, cwd: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = resolve_project_root(args.path, cwd)?;
    let mut manager = PackageManager::discover(&root)?;
    let installed = if args.packages.is_empty() {
        manager.install_all()?
    } else {
        manager.install_requests(&args.packages)?
    };
    for package in installed {
        println!(
            "installed {} {} ({})",
            package.package_id, package.version, package.repo
        );
    }
    Ok(())
}

fn run_remove(args: PackageArgs, cwd: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if args.packages.is_empty() {
        return Err(io::Error::other("remove expects one or more package aliases").into());
    }
    let root = resolve_project_root(args.path, cwd)?;
    let mut manager = PackageManager::discover(&root)?;
    manager.remove_aliases(&args.packages)?;
    for alias in args.packages {
        println!("removed {alias}");
    }
    Ok(())
}

fn run_update(args: PackageArgs, cwd: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = resolve_project_root(args.path, cwd)?;
    let mut manager = PackageManager::discover(&root)?;
    let updated = manager.update_requests(&args.packages)?;
    for package in updated {
        println!(
            "updated {} {} ({})",
            package.package_id, package.version, package.repo
        );
    }
    Ok(())
}

fn run_list(args: ProjectPathArgs, cwd: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = resolve_project_root(args.path, cwd)?;
    let manager = PackageManager::discover(&root)?;
    for package in manager.list()? {
        println!(
            "{} {} {}",
            package.package_id, package.version, package.repo
        );
    }
    Ok(())
}

fn run_bundle(args: BundleArgs, cwd: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = resolve_project_root(args.path, cwd)?;
    let manager = PackageManager::discover(&root)?;
    let bundle = manager.bundle(BundleOptions {
        minify: !args.no_minify,
    })?;
    println!("bundled {}", bundle.display());
    Ok(())
}

fn run_publish(args: PublishArgs, cwd: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = resolve_project_root(args.path, cwd)?;
    let manager = PackageManager::discover(&root)?;
    let validation = manager.validate_publish()?;
    println!(
        "validated {} {}",
        validation.manifest.name, validation.manifest.version
    );
    if !validation.exported_types.is_empty() {
        println!("exported types: {}", validation.exported_types.join(", "));
    }
    println!("public fields: {}", validation.public_fields.join(", "));
    if !args.dry_run {
        manager.publish_to_local_registry()?;
        println!("updated XLpkg registry repo index");
    }
    Ok(())
}

fn run_format(
    path: Option<PathBuf>,
    check: bool,
    cwd: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let files = resolve_format_targets(path, cwd)?;
    let mut changed = Vec::new();
    for file in files {
        let source = fs::read_to_string(&file)?;
        let formatted = format_source(&source);
        if formatted != source {
            changed.push(file.clone());
            if !check {
                fs::write(&file, formatted)?;
                println!("formatted {}", file.display());
            }
        }
    }

    if check && !changed.is_empty() {
        for file in &changed {
            eprintln!("needs formatting: {}", file.display());
        }
        return Err(io::Error::other("format check failed").into());
    }

    Ok(())
}

fn run_file(args: RunArgs, cwd: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let entry = resolve_run_entry(&args.path, cwd)?;
    let compiler_root = nearest_project_root(entry.parent().unwrap_or(cwd), cwd);
    let compiler = Compiler::discover(&compiler_root)?;
    let artifact = compiler.build_file(&entry)?;
    compiler.write_artifact(&artifact)?;

    let runtime = args
        .runtime
        .or_else(|| std::env::var("XLUAU_RUNTIME").ok())
        .unwrap_or_else(|| "luau".to_string());

    let status = ProcessCommand::new(&runtime)
        .arg(&artifact.output)
        .args(&args.args)
        .status()
        .map_err(|error| {
            io::Error::other(format!("failed to launch runtime `{runtime}`: {error}"))
        })?;
    if !status.success() {
        return Err(
            io::Error::other(format!("runtime `{runtime}` exited with status {status}")).into(),
        );
    }

    Ok(())
}

fn run_operation(
    operation: Operation,
    path: Option<PathBuf>,
    watch: bool,
    cwd: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let target = resolve_invocation_target(path, cwd)?;
    run_operation_once(operation, &target)?;
    if watch {
        watch_operation(operation, target)?;
    }
    Ok(())
}

fn run_operation_once(
    operation: Operation,
    target: &InvocationTarget,
) -> Result<(), Box<dyn std::error::Error>> {
    match target {
        InvocationTarget::ProjectRoot(root) => {
            let compiler = Compiler::discover(root)?;
            let artifacts = compiler.build_project()?;
            let project_rbxmx = compiler.build_project_rbxmx(&artifacts)?;
            for artifact in artifacts {
                match operation {
                    Operation::Build => {
                        compiler.write_artifact(&artifact)?;
                        println!(
                            "built {} -> {}",
                            artifact.input.display(),
                            artifact.output.display()
                        );
                    }
                    Operation::Check => {
                        println!(
                            "checked {} ({}) bytes",
                            artifact.input.display(),
                            artifact.luau.len()
                        );
                    }
                }
            }
            if let Some(project_rbxmx) = project_rbxmx {
                match operation {
                    Operation::Build => {
                        compiler.write_project_rbxmx(&project_rbxmx)?;
                        println!(
                            "built Roblox project export -> {}",
                            project_rbxmx.output.display()
                        );
                    }
                    Operation::Check => {
                        println!(
                            "checked Roblox project export -> {}",
                            project_rbxmx.output.display()
                        );
                    }
                }
            }
        }
        InvocationTarget::SingleFile {
            compiler_root,
            file_path,
        } => {
            let compiler = Compiler::discover(compiler_root)?;
            let artifact = compiler.build_file(file_path)?;
            match operation {
                Operation::Build => {
                    compiler.write_artifact(&artifact)?;
                    println!(
                        "built {} -> {}",
                        artifact.input.display(),
                        artifact.output.display()
                    );
                }
                Operation::Check => {
                    println!(
                        "checked {} ({}) bytes",
                        artifact.input.display(),
                        artifact.luau.len()
                    );
                }
            }
        }
    }
    Ok(())
}

fn watch_operation(
    operation: Operation,
    target: InvocationTarget,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut previous = snapshot_watch_state(&target)?;
    loop {
        thread::sleep(Duration::from_millis(750));
        let current = snapshot_watch_state(&target)?;
        if current == previous {
            continue;
        }
        previous = current;
        match run_operation_once(operation, &target) {
            Ok(()) => {}
            Err(error) => eprintln!("{error}"),
        }
    }
}

fn resolve_invocation_target(
    path: Option<PathBuf>,
    cwd: &Path,
) -> Result<InvocationTarget, Box<dyn std::error::Error>> {
    let Some(path) = path else {
        return Ok(InvocationTarget::ProjectRoot(cwd.to_path_buf()));
    };

    let absolute = absolutize(cwd, &path);
    if absolute.is_dir() {
        return Ok(InvocationTarget::ProjectRoot(absolute));
    }

    if is_config_path(&absolute) {
        let root = absolute.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("config path {} has no parent directory", absolute.display()),
            )
        })?;
        return Ok(InvocationTarget::ProjectRoot(root.to_path_buf()));
    }

    let compiler_root = nearest_project_root(absolute.parent().unwrap_or(cwd), cwd);

    Ok(InvocationTarget::SingleFile {
        compiler_root,
        file_path: absolute,
    })
}

fn absolutize(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn nearest_project_root(start: &Path, fallback: &Path) -> PathBuf {
    for ancestor in start.ancestors() {
        if ancestor.join("xluau.config.json").is_file() {
            return ancestor.to_path_buf();
        }
    }
    fallback.to_path_buf()
}

fn is_config_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case("xluau.config.json"))
        .unwrap_or(false)
}

fn resolve_format_targets(
    path: Option<PathBuf>,
    cwd: &Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let target = path
        .map(|path| absolutize(cwd, &path))
        .unwrap_or_else(|| cwd.to_path_buf());
    if target.is_file() && !is_config_path(&target) {
        return Ok(vec![target]);
    }

    let root = if target.is_dir() {
        target
    } else if is_config_path(&target) {
        target
            .parent()
            .ok_or_else(|| io::Error::other("config path has no parent"))?
            .to_path_buf()
    } else {
        cwd.to_path_buf()
    };

    let mut files = Vec::new();
    collect_format_files(&root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_format_files(
    root: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    if root.is_file() {
        if is_formattable_file(root) {
            files.push(root.to_path_buf());
        }
        return Ok(());
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some(".git") {
                continue;
            }
            collect_format_files(&path, files)?;
            continue;
        }
        if is_formattable_file(&path) {
            files.push(path);
        }
    }

    Ok(())
}

fn is_formattable_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("xl" | "luau" | "lua")
    )
}

fn resolve_run_entry(path: &Path, cwd: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let absolute = absolutize(cwd, path);
    if absolute.is_file() && !is_config_path(&absolute) {
        return Ok(absolute);
    }

    let root = if absolute.is_dir() {
        absolute
    } else if is_config_path(&absolute) {
        absolute
            .parent()
            .ok_or_else(|| io::Error::other("config path has no parent"))?
            .to_path_buf()
    } else {
        return Err(io::Error::other(format!(
            "run expects a source file, project directory, or config path: {}",
            absolute.display()
        ))
        .into());
    };

    for candidate in [
        root.join("src/main.xl"),
        root.join("src/main.luau"),
        root.join("main.xl"),
        root.join("main.luau"),
    ] {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(io::Error::other(format!(
        "could not find a runnable entry file in {}",
        root.display()
    ))
    .into())
}

fn resolve_project_root(
    path: Option<PathBuf>,
    cwd: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let target = resolve_invocation_target(path, cwd)?;
    match target {
        InvocationTarget::ProjectRoot(root) => Ok(root),
        InvocationTarget::SingleFile { compiler_root, .. } => Ok(compiler_root),
    }
}

fn snapshot_watch_state(
    target: &InvocationTarget,
) -> Result<Vec<(PathBuf, Option<SystemTime>)>, Box<dyn std::error::Error>> {
    let root = match target {
        InvocationTarget::ProjectRoot(root) => root.clone(),
        InvocationTarget::SingleFile {
            compiler_root,
            file_path,
        } => {
            let mut files = vec![file_path.clone()];
            let config = compiler_root.join("xluau.config.json");
            if config.is_file() {
                files.push(config);
            }
            return Ok(snapshot_files(files));
        }
    };
    let mut files = Vec::new();
    collect_watch_files(&root, &mut files)?;
    Ok(snapshot_files(files))
}

fn collect_watch_files(
    root: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    if root.is_file() {
        files.push(root.to_path_buf());
        return Ok(());
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_watch_files(&path, files)?;
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        if matches!(ext, "xl" | "luau" | "lua" | "json") || name == "xluau.config.json" {
            files.push(path);
        }
    }

    files.sort();
    Ok(())
}

fn snapshot_files(files: Vec<PathBuf>) -> Vec<(PathBuf, Option<SystemTime>)> {
    files
        .into_iter()
        .map(|path| {
            let modified = fs::metadata(&path).and_then(|meta| meta.modified()).ok();
            (path, modified)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        InvocationTarget, nearest_project_root, resolve_format_targets, resolve_invocation_target,
        resolve_run_entry,
    };

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("xluau_cli_{name}_{nonce}"));
        fs::create_dir_all(&root).expect("temp dir");
        root
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(path, contents).expect("write");
    }

    #[test]
    fn resolves_directory_as_project_root() {
        let root = temp_dir("dir_project");
        write_file(&root.join("xluau.config.json"), "{}");

        let target =
            resolve_invocation_target(Some(root.clone()), Path::new("D:/unused")).expect("target");
        match target {
            InvocationTarget::ProjectRoot(found) => assert_eq!(found, root),
            other => panic!("expected project root, got {other:?}"),
        }
    }

    #[test]
    fn resolves_config_file_as_project_root() {
        let root = temp_dir("config_project");
        let config = root.join("xluau.config.json");
        write_file(&config, "{}");

        let target =
            resolve_invocation_target(Some(config), Path::new("D:/unused")).expect("target");
        match target {
            InvocationTarget::ProjectRoot(found) => assert_eq!(found, root),
            other => panic!("expected project root, got {other:?}"),
        }
    }

    #[test]
    fn resolves_file_to_nearest_project_root() {
        let root = temp_dir("nearest_project");
        write_file(&root.join("xluau.config.json"), "{}");
        let source = root.join("src/nested/main.xl");
        write_file(&source, "return nil");

        let target = resolve_invocation_target(Some(source.clone()), Path::new("D:/unused"))
            .expect("target");
        match target {
            InvocationTarget::SingleFile {
                compiler_root,
                file_path,
            } => {
                assert_eq!(compiler_root, root);
                assert_eq!(file_path, source);
            }
            other => panic!("expected single file, got {other:?}"),
        }
    }

    #[test]
    fn falls_back_to_cwd_when_no_project_config_exists() {
        let cwd = temp_dir("cwd_fallback");
        let file_root = temp_dir("no_project");
        let source = file_root.join("main.xl");
        write_file(&source, "return nil");

        let target = resolve_invocation_target(Some(source.clone()), &cwd).expect("target");
        match target {
            InvocationTarget::SingleFile {
                compiler_root,
                file_path,
            } => {
                assert_eq!(compiler_root, cwd);
                assert_eq!(file_path, source);
            }
            other => panic!("expected single file, got {other:?}"),
        }
    }

    #[test]
    fn finds_nearest_project_root() {
        let root = temp_dir("find_root");
        write_file(&root.join("xluau.config.json"), "{}");
        let nested = root.join("src/server/controllers");
        fs::create_dir_all(&nested).expect("nested");

        assert_eq!(
            nearest_project_root(&nested, Path::new("D:/fallback")),
            root
        );
    }

    #[test]
    fn resolves_format_targets_for_project_tree() {
        let root = temp_dir("fmt_targets");
        write_file(&root.join("src/main.xl"), "return nil");
        write_file(&root.join("src/util.luau"), "return nil");
        write_file(&root.join("README.md"), "ignored");

        let files = resolve_format_targets(Some(root.clone()), Path::new("D:/unused"))
            .expect("format targets");
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|path| path.ends_with("src/main.xl")));
        assert!(files.iter().any(|path| path.ends_with("src/util.luau")));
    }

    #[test]
    fn resolves_run_entry_from_project_directory() {
        let root = temp_dir("run_entry");
        let main = root.join("src/main.xl");
        write_file(&main, "return nil");

        let entry = resolve_run_entry(&root, Path::new("D:/unused")).expect("entry");
        assert_eq!(entry, main);
    }
}
