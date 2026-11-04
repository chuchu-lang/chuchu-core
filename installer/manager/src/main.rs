//! Installer, stable launcher and explicit updater. No LLVM or Python required.
mod platform;
mod release;

use release::{Manifest, State};
use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const UPDATE_URL: &str = match option_env!("CHEEKU_UPDATE_URL") {
    Some(url) => url,
    None => "https://downloads.chuchu-lang.org/latest.json",
};
const HELP: &str = "Chuchu installer and launcher\n\
Usage: cheeku install [--manifest <HTTPS URL or local JSON>] [--prefix <directory>] [--no-path] [--skip-dependencies]\n\
       cheeku update [--check] [--manifest <HTTPS URL or local JSON>]\n\
       cheeku --version\n\
       cheeku <source.chuchu> [compiler options]\n\
Compiler options: --debug, -g, -O0/-O1/-O2/-O3, -o <executable>, --cpu generic/native,\n\
--time-compilation, --emit-ir <file>, --emit-assembly <file>, --no-vectorize.\n\
Installation is per-user. Updates are explicit; previous compiler versions are retained.";

#[derive(Default, Debug)]
struct Options {
    manifest: Option<String>,
    prefix: Option<PathBuf>,
    no_path: bool,
    skip_dependencies: bool,
    check: bool,
}
fn options(args: &[OsString], install: bool) -> Result<Options> {
    let mut result = Options::default();
    let mut seen = std::collections::HashSet::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let arg = arg.to_str().ok_or("Non-UTF-8 option")?;
        if !seen.insert(arg) {
            return Err(format!("Duplicate option: {arg}").into());
        }
        match arg {
            "--manifest" => {
                result.manifest = Some(
                    args.next()
                        .and_then(|s| s.to_str())
                        .filter(|s| !s.is_empty())
                        .ok_or("--manifest needs a value")?
                        .into(),
                )
            }
            "--prefix" if install => {
                result.prefix = Some(
                    args.next()
                        .filter(|s| !s.is_empty())
                        .ok_or("--prefix needs a directory")?
                        .into(),
                )
            }
            "--no-path" if install => result.no_path = true,
            "--skip-dependencies" if install => result.skip_dependencies = true,
            "--check" if !install => result.check = true,
            _ => return Err(format!("Unknown option: {arg}\n{HELP}").into()),
        }
    }
    Ok(result)
}
fn default_prefix() -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    let prefix =
        PathBuf::from(env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?).join("Chuchu");
    #[cfg(target_os = "macos")]
    let prefix = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?)
        .join("Library/Application Support/Chuchu");
    #[cfg(target_os = "linux")]
    let prefix = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or(
            PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?).join(".local/share"),
        )
        .join("chuchu");
    Ok(prefix)
}
fn installed_prefix() -> Result<PathBuf> {
    let executable = env::current_exe()?;
    let prefix = executable
        .parent()
        .and_then(Path::parent)
        .ok_or("Cannot locate installation")?;
    if prefix.join("current.json").is_file() {
        Ok(prefix.into())
    } else {
        default_prefix()
    }
}
fn refresh_launcher(prefix: &Path, no_path: bool, replace: bool) -> Result<()> {
    let bin = prefix.join("bin");
    fs::create_dir_all(&bin)?;
    let launcher = bin.join(if cfg!(windows) {
        "cheeku.exe"
    } else {
        "cheeku"
    });
    let executable = env::current_exe()?;
    let same = fs::canonicalize(&launcher).ok() == fs::canonicalize(&executable).ok();
    if !launcher.exists() || (replace && !same) {
        let pending = tempfile::NamedTempFile::new_in(&bin)?;
        fs::copy(executable, pending.path())?;
        pending.as_file().sync_all()?;
        pending.persist(&launcher).map_err(|e| e.error)?;
    }
    if !no_path {
        platform::add_to_path(&bin)?;
    }
    Ok(())
}
fn install(opts: Options, update: bool) -> Result<()> {
    let prefix = if let Some(prefix) = opts.prefix {
        env::current_dir()?.join(prefix)
    } else if update {
        installed_prefix()?
    } else {
        default_prefix()?
    };
    let previous = State::read(&prefix)?;
    let source = opts.manifest.unwrap_or_else(|| {
        previous
            .as_ref()
            .map(|s| s.manifest.clone())
            .unwrap_or_else(|| UPDATE_URL.into())
    });
    let manifest = Manifest::read(&source)?;
    let source = if source.starts_with("https://") {
        source
    } else {
        fs::canonicalize(&source)?
            .into_os_string()
            .into_string()
            .map_err(|_| "Non-UTF-8 manifest path")?
    };
    if let Some(state) = &previous {
        let old = semver::Version::parse(&state.version)?;
        let new = semver::Version::parse(&manifest.version)?;
        if new <= old {
            if new == old && !update {
                refresh_launcher(&prefix, opts.no_path, true)?;
                State {
                    version: state.version.clone(),
                    manifest: manifest.update_url.clone().unwrap_or(source),
                }
                .write(&prefix)?;
                println!(
                    "Refreshed the launcher for Chuchu {}. Open a new terminal/application session.",
                    state.version
                );
            } else {
                println!(
                    "Chuchu {} is installed; no newer version is available.",
                    state.version
                );
            }
            return Ok(());
        }
    }
    if opts.check {
        println!("Chuchu {} is available.", manifest.version);
        return Ok(());
    }
    // Select a supported target before asking for or installing any OS components.
    let asset = manifest.asset()?;
    if !opts.skip_dependencies {
        platform::ensure_dependencies()?;
    }
    fs::create_dir_all(&prefix)?;
    let lock_path = prefix.join("install.lock");
    let lock = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|e| {
            format!(
                "Another install/update may be active ({}): {e}",
                lock_path.display()
            )
        })?;
    let mut staged_version = None;
    let result = (|| {
        let version = release::stage(&prefix, &manifest.version, asset, &source)?;
        staged_version = Some(version.clone());
        let compiler = version.join("bin").join(platform::compiler_name());
        let mut command = Command::new(&compiler);
        platform::configure_compiler(&mut command, &version)?;
        let output = command.arg("--version").output()?;
        if !output.status.success()
            || String::from_utf8_lossy(&output.stdout).trim()
                != format!("cheeku {}", manifest.version)
        {
            return Err("Downloaded compiler did not report the expected version; installation was not activated".into());
        }
        // Verify the installed, relocated toolchain before changing the active version.
        let smoke = tempfile::tempdir()?;
        let source_file = smoke.path().join("hello.chuchu");
        let executable = smoke
            .path()
            .join(if cfg!(windows) { "hello.exe" } else { "hello" });
        fs::write(&source_file, "show 6 * 7\n")?;
        let mut build = Command::new(&compiler);
        platform::configure_compiler(&mut build, &version)?;
        if !build
            .arg(&source_file)
            .arg("-o")
            .arg(&executable)
            .status()?
            .success()
        {
            return Err(
                "Installed compiler could not build hello-world; previous version remains active"
                    .into(),
            );
        }
        let output = Command::new(executable).output()?;
        if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != "42" {
            return Err("Installed hello-world failed; previous version remains active".into());
        }
        refresh_launcher(&prefix, opts.no_path, !update)?;
        State {
            version: manifest.version.clone(),
            manifest: manifest.update_url.clone().unwrap_or(source),
        }
        .write(&prefix)?;
        println!(
            "Installed Chuchu {} in {}.",
            manifest.version,
            prefix.display()
        );
        println!("Open a new terminal, then run: cheeku --version");
        println!(
            "Compile: cheeku hello.chuchu -o hello{}",
            if cfg!(windows) { ".exe" } else { "" }
        );
        println!("Update: cheeku update");
        Ok(())
    })();
    if result.is_err()
        && let Some(version) = staged_version
    {
        let _ = fs::remove_dir_all(version);
    }
    drop(lock);
    fs::remove_file(lock_path)?;
    result
}
fn launch(args: &[OsString]) -> Result<i32> {
    let prefix = installed_prefix()?;
    let state = State::read(&prefix)?
        .ok_or("Chuchu is not installed. Run the installer or cheeku install first.")?;
    if args == [OsString::from("--version")] || args == [OsString::from("-V")] {
        println!("cheeku {}", state.version);
        return Ok(0);
    }
    let version = prefix.join("versions").join(&state.version);
    let mut command = Command::new(version.join("bin").join(platform::compiler_name()));
    platform::configure_compiler(&mut command, &version)?;
    Ok(command.args(args).status()?.code().unwrap_or(1))
}
fn run() -> Result<i32> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match args.first().and_then(|a| a.to_str()) {
        Some("install") => {
            install(options(&args[1..], true)?, false)?;
            Ok(0)
        }
        Some("update") => {
            install(options(&args[1..], false)?, true)?;
            Ok(0)
        }
        Some("--manager-update-url") if args.len() == 1 => {
            println!("{UPDATE_URL}");
            Ok(0)
        }
        Some("--manager-help" | "--help" | "-h") => {
            println!("{HELP}");
            Ok(0)
        }
        None if env::current_exe()?
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("Chuchu-Setup-"))
            || State::read(&installed_prefix()?)?.is_none() =>
        {
            install(Options::default(), false)?;
            Ok(0)
        }
        _ => launch(&args),
    }
}
fn main() -> ExitCode {
    let outcome = run();
    #[cfg(target_os = "windows")]
    if env::args_os().len() == 1
        && env::current_exe()
            .ok()
            .and_then(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with("Chuchu-Setup-"))
            })
            .unwrap_or(false)
    {
        if let Err(error) = &outcome {
            eprintln!("cheeku: {error}");
        }
        println!("Press Enter to close the installer.");
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    match outcome {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            eprintln!("cheeku: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn rejects_ambiguous_or_invalid_installer_options() {
        for values in [
            vec!["--prefix"],
            vec!["--manifest"],
            vec!["--check"],
            vec!["--no-path", "--no-path"],
        ] {
            assert!(options(&args(&values), true).is_err());
        }
        assert!(options(&args(&["--prefix", "/tmp"]), false).is_err());
        assert!(options(&args(&["--check"]), false).unwrap().check);
    }
}
