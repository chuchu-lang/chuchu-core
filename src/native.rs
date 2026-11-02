use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::optimization::{self, BuildOptions, DebugInfo, OptimizationError};
use inkwell::module::Module;
use inkwell::targets::FileType;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum NativeError {
    Target(String),
    Optimization(OptimizationError),
    Verification(String),
    ObjectEmission(String),
    DebugSymbols(String),
    Filesystem { path: PathBuf, source: io::Error },
    LinkerUnavailable(io::Error),
    LinkerFailed { status: Option<i32>, stderr: String },
}

impl fmt::Display for NativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Optimization(error) => error.fmt(f),
            Self::Target(message) => write!(f, "LLVM host target initialization failed: {message}"),
            Self::Verification(message) => write!(f, "LLVM module verification failed: {message}"),
            Self::DebugSymbols(message) => {
                write!(f, "Debug symbol generation/publication failed: {message}")
            }
            Self::ObjectEmission(message) => write!(f, "LLVM object emission failed: {message}"),
            Self::Filesystem { path, source } => {
                write!(f, "Cannot access {}: {source}", path.display())
            }
            Self::LinkerUnavailable(source) => write!(
                f,
                "Cannot start the system Clang linker driver ({}): {source}",
                linker_hint()
            ),
            Self::LinkerFailed { status, stderr } => write!(
                f,
                "Clang linker failed (exit status {status:?}): {}",
                stderr.trim()
            ),
        }
    }
}

impl std::error::Error for NativeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Filesystem { source, .. } | Self::LinkerUnavailable(source) => Some(source),
            Self::Optimization(error) => Some(error),
            _ => None,
        }
    }
}

fn filesystem_error(path: &Path, source: io::Error) -> NativeError {
    NativeError::Filesystem {
        path: path.to_owned(),
        source,
    }
}

#[derive(Debug, Default, Clone)]
pub struct NativeTimings {
    pub target_setup: Duration,
    pub optimization: Duration,
    pub object: Duration,
    pub link: Duration,
}

/// Default CLI output; explicitly requested paths are never rewritten.
#[cfg(target_os = "windows")]
pub const DEFAULT_EXECUTABLE_NAME: &str = "main.exe";
#[cfg(not(target_os = "windows"))]
pub const DEFAULT_EXECUTABLE_NAME: &str = "main";

/// Persistent symbols associated with an executable. Append rather than replace
/// its extension so custom executable filenames always have distinct sidecars.
pub fn debug_symbol_path(output: &Path) -> Option<PathBuf> {
    let suffix = if cfg!(target_os = "macos") {
        ".dSYM"
    } else if cfg!(target_os = "windows") {
        ".pdb"
    } else {
        return None;
    };
    let mut path = output.as_os_str().to_owned();
    path.push(suffix);
    Some(path.into())
}

fn linker_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "install Xcode command line tools (xcrun clang)"
    } else if cfg!(target_os = "windows") {
        "install LLVM clang-cl and Visual Studio C++ Build Tools with the Windows SDK; use an x64 Developer PowerShell"
    } else {
        "install Clang, a GNU linker, and the C library development packages"
    }
}

/// One executable path, never a shell command. Useful for controlled toolchain selection.
fn configured_linker() -> Option<OsString> {
    if let Some(linker) = std::env::var_os("CHEEKU_LINKER") {
        return Some(linker);
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LLVM_SYS_231_PREFIX")
            .map(|prefix| PathBuf::from(prefix).join("bin").join("clang-cl.exe"))
            .filter(|path| path.is_file())
            .map(Into::into)
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// Compatibility object API; new programs use the default O2 policy.
pub fn emit_object(module: &Module<'_>, output: &Path) -> Result<(), NativeError> {
    emit_object_with_options(module, output, &BuildOptions::default()).map(|_| ())
}

pub fn emit_object_with_options(
    module: &Module<'_>,
    output: &Path,
    options: &BuildOptions,
) -> Result<NativeTimings, NativeError> {
    module
        .verify()
        .map_err(|error| NativeError::Verification(error.to_string()))?;
    let start = Instant::now();
    let machine =
        optimization::target_machine(module, options).map_err(NativeError::Optimization)?;
    let target_setup = start.elapsed();
    let start = Instant::now();
    optimization::optimize(module, &machine, options).map_err(NativeError::Optimization)?;
    let optimization = start.elapsed();
    if let Some(path) = &options.emit_ir {
        fs::write(path, module.print_to_string().to_bytes())
            .map_err(|error| filesystem_error(path, error))?;
    }
    if let Some(path) = &options.emit_assembly {
        let assembly = machine
            .write_to_memory_buffer(module, FileType::Assembly)
            .map_err(|error| NativeError::ObjectEmission(error.to_string()))?;
        fs::write(path, assembly.as_slice()).map_err(|error| filesystem_error(path, error))?;
    }
    let start = Instant::now();
    module
        .verify()
        .map_err(|error| NativeError::Verification(error.to_string()))?;
    let object = machine
        .write_to_memory_buffer(module, FileType::Object)
        .map_err(|error| NativeError::ObjectEmission(error.to_string()))?;
    fs::write(output, object.as_slice()).map_err(|error| filesystem_error(output, error))?;
    Ok(NativeTimings {
        target_setup,
        optimization,
        object: start.elapsed(),
        link: Duration::ZERO,
    })
}

/// Uses the platform Clang driver with separate arguments, never a shell.
/// Windows requires an x64 MSVC developer environment (LIB/INCLUDE/SDK).
pub fn link_object(object: &Path, output: &Path) -> Result<(), NativeError> {
    let object = std::path::absolute(object).map_err(|error| filesystem_error(object, error))?;
    let output = std::path::absolute(output).map_err(|error| filesystem_error(output, error))?;
    run_linker(linker_command(&object, &output))
}

fn run_linker(mut command: Command) -> Result<(), NativeError> {
    let result = command.output().map_err(NativeError::LinkerUnavailable)?;
    if !result.status.success() {
        // Some drivers report useful diagnostics on stdout (notably MSVC).
        let diagnostics = format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return Err(NativeError::LinkerFailed {
            status: result.status.code(),
            stderr: format!("{diagnostics}\nToolchain: {}", linker_hint()),
        });
    }
    Ok(())
}

fn linker_command(object: &Path, output: &Path) -> Command {
    linker_command_with_runtime(object, output, None)
}

fn linker_command_with_runtime(object: &Path, output: &Path, runtime: Option<&Path>) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut command =
            Command::new(configured_linker().unwrap_or_else(|| "clang-cl.exe".into()));
        let mut destination = OsString::from("/Fe");
        destination.push(output);
        command.args(["/nologo", "/MD"]).arg(object);
        if let Some(runtime) = runtime {
            // clang-cl otherwise writes its compiled C object into the current
            // directory, colliding across concurrent compilations. /Fo must name
            // a directory (trailing separator) because clang-cl rejects a file
            // path when compiling multiple sources.
            let runtime_dir = runtime
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let mut runtime_object = OsString::from("/Fo");
            runtime_object.push(runtime_dir);
            runtime_object.push("/");
            command
                .args(["/O2", "/std:c11"])
                .arg(runtime_object)
                .arg(runtime);
        }
        // The driver derives `-out:` from /Fe and appends .exe when the name has
        // no extension; a later /OUT: wins, preserving the exact output path.
        let mut link_output = OsString::from("/OUT:");
        link_output.push(output);
        command
            .arg(destination)
            .arg("/link")
            .arg(link_output)
            .args([
                "/MACHINE:X64",
                "/SUBSYSTEM:CONSOLE",
                "/DEFAULTLIB:msvcrt.lib",
                "/DEFAULTLIB:legacy_stdio_definitions.lib",
            ]);
        command
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut command = if let Some(linker) = configured_linker() {
            Command::new(linker)
        } else if cfg!(target_os = "macos") {
            let mut driver = Command::new("xcrun");
            driver.arg("clang");
            driver
        } else {
            Command::new("clang")
        };
        // link_object normalizes both paths to absolute paths, so they cannot
        // be mistaken for options. Keep libm after the object for --as-needed.
        command.arg("-o").arg(output).arg(object);
        if let Some(runtime) = runtime {
            command.args(["-O2", "-std=c11"]).arg(runtime);
        }
        #[cfg(target_os = "linux")]
        command.arg("-lm");
        command
    }
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TemporaryDirectory(PathBuf, bool);

impl TemporaryDirectory {
    fn create(parent: &Path) -> Result<Self, NativeError> {
        for _ in 0..100 {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".cheeku-{}-{id}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path, false)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(filesystem_error(&path, error)),
            }
        }
        Err(filesystem_error(
            parent,
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Cannot allocate a temporary compilation directory",
            ),
        ))
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        if !self.1 {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

/// Stages object and executable beside the output, publishing only after linking succeeds.
pub fn build_executable(module: &Module<'_>, output: &Path) -> Result<(), NativeError> {
    build_executable_with_options(module, output, &BuildOptions::default()).map(|_| ())
}

pub fn build_executable_with_options(
    module: &Module<'_>,
    output: &Path,
    options: &BuildOptions,
) -> Result<NativeTimings, NativeError> {
    let output = std::path::absolute(output).map_err(|error| filesystem_error(output, error))?;
    let parent = output.parent().ok_or_else(|| {
        filesystem_error(
            &output,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Output must name an executable file",
            ),
        )
    })?;
    let mut temporary = TemporaryDirectory::create(parent)?;
    let object = temporary.0.join(if cfg!(windows) {
        "program.obj"
    } else {
        "program.o"
    });
    let artifacts = temporary.0.join("artifacts");
    fs::create_dir(&artifacts).map_err(|error| filesystem_error(&artifacts, error))?;
    // Keep the final basename inside a macOS dSYM's DWARF directory, in a
    // separate directory so an output named program.o cannot collide with input.
    let executable = artifacts.join(output.file_name().ok_or_else(|| {
        filesystem_error(
            &output,
            io::Error::new(io::ErrorKind::InvalidInput, "Missing executable filename"),
        )
    })?);
    let mut timings = emit_object_with_options(module, &object, options)?;
    let start = Instant::now();
    let symbols = if options.debug_info == DebugInfo::Full {
        debug_symbol_path(&output)
    } else {
        None
    };
    let staged_symbols = symbols
        .as_ref()
        .map(|_| debug_symbol_path(&executable).expect("host sidecar"));
    let runtime = if crate::runtime::FUNCTIONS
        .iter()
        .any(|name| module.get_function(name).is_some())
    {
        let path = temporary.0.join("cheeku-runtime.c");
        fs::write(&path, crate::runtime::c_source())
            .map_err(|error| filesystem_error(&path, error))?;
        Some(path)
    } else {
        None
    };
    let command = linker_command_with_runtime(&object, &executable, runtime.as_deref());
    #[cfg(target_os = "windows")]
    let command = {
        let mut command = command;
        if let (Some(staged), Some(destination)) = (&staged_symbols, &symbols) {
            command.arg("/DEBUG");
            let mut pdb = OsString::from("/PDB:");
            pdb.push(staged);
            command.arg(pdb);
            let mut location = OsString::from("/PDBALTPATH:");
            location.push(destination);
            command.arg(location);
        }
        command
    };
    run_linker(command)?;
    #[cfg(target_os = "macos")]
    if let Some(staged) = &staged_symbols {
        let mut tool = if let Some(configured) = std::env::var_os("CHEEKU_DSYMUTIL") {
            Command::new(configured)
        } else if let Some(tool) = std::env::var_os("LLVM_SYS_231_PREFIX")
            .map(|prefix| PathBuf::from(prefix).join("bin/dsymutil"))
            .filter(|path| path.is_file())
        {
            Command::new(tool)
        } else {
            let mut tool = Command::new("xcrun");
            tool.arg("dsymutil");
            tool
        };
        let result = tool
            .arg(&executable)
            .arg("-o")
            .arg(staged)
            .output()
            .map_err(|error| {
                NativeError::DebugSymbols(format!("Cannot start dsymutil: {error}"))
            })?;
        if !result.status.success() {
            return Err(NativeError::DebugSymbols(format!(
                "dsymutil: {}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            )));
        }
    }
    if let (Some(staged), Some(destination)) = (&staged_symbols, &symbols) {
        if !staged.exists() {
            return Err(NativeError::DebugSymbols(format!(
                "Symbol tool produced no {}",
                staged.display()
            )));
        }
        publish_debug_artifacts(&executable, &output, staged, destination, &mut temporary)?;
    } else {
        fs::rename(&executable, &output).map_err(|error| filesystem_error(&output, error))?;
    }
    timings.link = start.elapsed();
    Ok(timings)
}

/// Publish symbols first and replace the executable last. A failed executable
/// replacement rolls back symbols. If rollback itself fails, preserve staging
/// (including the old symbols) and report its location for manual recovery.
fn publish_debug_artifacts(
    executable: &Path,
    output: &Path,
    staged: &Path,
    destination: &Path,
    temporary: &mut TemporaryDirectory,
) -> Result<(), NativeError> {
    if output.is_dir() {
        return Err(filesystem_error(
            output,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Executable destination is a directory",
            ),
        ));
    }
    let backup = temporary.0.join("previous-symbols");
    let had_symbols = fs::symlink_metadata(destination).is_ok();
    if had_symbols {
        fs::rename(destination, &backup).map_err(|error| filesystem_error(destination, error))?;
    }
    let mut installed = false;
    let publication = (|| {
        fs::rename(staged, destination).map_err(|error| filesystem_error(destination, error))?;
        installed = true;
        fs::rename(executable, output).map_err(|error| filesystem_error(output, error))
    })();
    if let Err(error) = publication {
        let rollback = (|| -> io::Result<()> {
            if installed {
                fs::rename(destination, staged)?;
            }
            if had_symbols {
                fs::rename(&backup, destination)?;
            }
            Ok(())
        })();
        if let Err(rollback) = rollback {
            temporary.1 = true;
            return Err(NativeError::DebugSymbols(format!(
                "{error}; rollback failed: {rollback}; recovery files preserved at {}",
                temporary.0.display()
            )));
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_executable_publication_restores_previous_symbols() {
        let mut temporary = TemporaryDirectory::create(&std::env::temp_dir()).unwrap();
        let output = temporary.0.join("working-program");
        let destination = temporary.0.join("working-symbols");
        let staged = temporary.0.join("new-symbols");
        fs::write(&output, "previous executable").unwrap();
        fs::write(&destination, "previous symbols").unwrap();
        fs::write(&staged, "new symbols").unwrap();
        let missing = temporary.0.join("missing-executable");
        assert!(
            publish_debug_artifacts(&missing, &output, &staged, &destination, &mut temporary)
                .is_err()
        );
        assert_eq!(fs::read_to_string(&output).unwrap(), "previous executable");
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "previous symbols"
        );
        assert_eq!(fs::read_to_string(&staged).unwrap(), "new symbols");
        assert!(!temporary.1);
    }

    #[test]
    fn linker_keeps_paths_as_individual_arguments() {
        let object = Path::new("object with spaces.obj");
        let output = Path::new("program with spaces.exe");
        let command = linker_command(object, output);
        let args: Vec<_> = command.get_args().collect();
        assert!(args.contains(&object.as_os_str()));
        #[cfg(not(windows))]
        {
            assert!(args.contains(&output.as_os_str()));
            #[cfg(target_os = "linux")]
            assert!(args.contains(&std::ffi::OsStr::new("-lm")));
        }
        #[cfg(windows)]
        assert!(args.contains(&std::ffi::OsStr::new("/Feprogram with spaces.exe")));
    }

    #[test]
    fn linker_compiles_runtime_source_with_spaces_before_link_options() {
        let runtime = Path::new("runtime with spaces.c");
        let command = linker_command_with_runtime(
            Path::new("program.obj"),
            Path::new("program.exe"),
            Some(runtime),
        );
        let args: Vec<_> = command.get_args().collect();
        assert!(args.contains(&runtime.as_os_str()));
        #[cfg(windows)]
        {
            let source = args
                .iter()
                .position(|arg| *arg == runtime.as_os_str())
                .unwrap();
            let link = args.iter().position(|arg| *arg == "/link").unwrap();
            assert!(source < link);
            assert!(args.contains(&std::ffi::OsStr::new("/std:c11")));
            assert!(args.contains(&std::ffi::OsStr::new("/Fo./")));

            let nested = Path::new("build dir/runtime with spaces.c");
            let command = linker_command_with_runtime(
                Path::new("program.obj"),
                Path::new("program.exe"),
                Some(nested),
            );
            let args: Vec<_> = command.get_args().collect();
            assert!(args.contains(&nested.as_os_str()));
            assert!(args.contains(&std::ffi::OsStr::new("/Fobuild dir/")));
        }
        #[cfg(not(windows))]
        assert!(args.contains(&std::ffi::OsStr::new("-std=c11")));
    }

    #[cfg(windows)]
    #[test]
    fn linker_passes_exact_output_path_to_link() {
        for output in ["program with spaces.exe", "exact-name"] {
            let command = linker_command_with_runtime(
                Path::new("program.obj"),
                Path::new(output),
                Some(Path::new("runtime.c")),
            );
            let args: Vec<_> = command.get_args().collect();
            let link = args.iter().position(|arg| *arg == "/link").unwrap();
            let expected = format!("/OUT:{output}");
            let out = args
                .iter()
                .position(|arg| *arg == expected.as_str())
                .unwrap();
            assert!(link < out);
        }
    }
}
