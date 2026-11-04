use crate::Result;
#[cfg(not(target_os = "windows"))]
use std::fs;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub fn compiler_name() -> &'static str {
    if cfg!(windows) {
        "cheeku.exe"
    } else {
        "cheeku"
    }
}
#[cfg(not(target_os = "windows"))]
fn succeeds(command: &mut Command) -> bool {
    command.output().is_ok_and(|o| o.status.success())
}
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run(command: &mut Command) -> Result<()> {
    if !command.status()?.success() {
        return Err("OS toolchain setup failed. Resolve the installer error and run the Chuchu installer again.".into());
    }
    Ok(())
}
pub fn ensure_dependencies() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if !succeeds(Command::new("xcrun").args(["--find", "clang"])) {
            let _ = Command::new("xcode-select").arg("--install").status();
            return Err("Finish Apple's Command Line Tools installation, then run this installer again. Rust and a separate LLVM installation are not required.".into());
        }
    }
    #[cfg(target_os = "linux")]
    {
        if !succeeds(Command::new("cc").arg("--version"))
            || !Path::new("/usr/include/stdio.h").is_file()
        {
            if succeeds(Command::new("apt-get").arg("--version")) {
                run(Command::new("sudo").args(["apt-get", "update"]))?;
                run(Command::new("sudo").args(["apt-get", "install", "-y", "build-essential"]))?;
            } else if succeeds(Command::new("dnf").arg("--version")) {
                run(Command::new("sudo").args([
                    "dnf",
                    "install",
                    "-y",
                    "gcc",
                    "glibc-devel",
                    "libstdc++",
                ]))?;
            } else {
                return Err("Install a C compiler/linker and libc development headers for your distribution, then rerun this installer.".into());
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        if developer_shell().is_err() {
            run(Command::new("winget").args(["install", "--id", "Microsoft.VisualStudio.2022.BuildTools", "--exact", "--accept-source-agreements", "--accept-package-agreements", "--override", "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --norestart"]))?;
            developer_shell()?;
        }
    }
    Ok(())
}
#[cfg(target_os = "windows")]
fn developer_shell() -> Result<PathBuf> {
    let vswhere =
        PathBuf::from(std::env::var_os("ProgramFiles(x86)").ok_or("ProgramFiles(x86) is not set")?)
            .join("Microsoft Visual Studio/Installer/vswhere.exe");
    let output = Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()?;
    let path =
        PathBuf::from(String::from_utf8(output.stdout)?.trim()).join("Common7/Tools/VsDevCmd.bat");
    if !output.status.success() || !path.is_file() {
        return Err("Visual Studio C++ Build Tools and the Windows SDK are required".into());
    }
    Ok(path)
}
pub fn configure_compiler(command: &mut Command, version: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let driver = if Path::new("/usr/bin/clang").is_file() {
            "/usr/bin/clang"
        } else {
            "/usr/bin/cc"
        };
        command.env("CHEEKU_LINKER", driver);
    }
    #[cfg(target_os = "windows")]
    {
        let shell = developer_shell()?;
        // VsDevCmd is vendor-installed. Reject shell metacharacters before invoking cmd.
        let path = shell.to_str().ok_or("Non-UTF-8 VS toolchain path")?;
        if path.contains(['"', '%', '!', '&', '|', '<', '>', '^', '\r', '\n']) {
            return Err("Unsupported character in Visual Studio installation path".into());
        }
        let output = Command::new("cmd.exe")
            .args([
                "/d",
                "/u",
                "/s",
                "/c",
                &format!("\"\"{path}\" -arch=x64 -host_arch=x64 >nul && set\""),
            ])
            .output()?;
        if !output.status.success() {
            return Err("Cannot initialize the Visual Studio toolchain".into());
        }
        if output.stdout.len() % 2 != 0 {
            return Err("Invalid UTF-16 toolchain environment".into());
        }
        let units: Vec<u16> = output
            .stdout
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        let environment = String::from_utf16(&units)?;
        for line in environment.lines() {
            if let Some((name, value)) = line.split_once('=')
                && !name.is_empty()
            {
                command.env(name, value);
            }
        }
        command.env("CHEEKU_LINKER", version.join("tools/clang-cl.exe"));
    }
    let _ = (command, version);
    Ok(())
}
#[cfg(not(target_os = "windows"))]
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
pub fn add_to_path(bin: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use winreg::{
            RegKey,
            enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE},
        };
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)?;
        let old: String = key.get_value("Path").unwrap_or_default();
        let bin = bin.to_str().ok_or("Non-UTF-8 install directory")?;
        if !old.split(';').any(|p| p.eq_ignore_ascii_case(bin)) {
            let value = format!("{bin};{old}\0");
            key.set_raw_value(
                "Path",
                &winreg::RegValue {
                    bytes: value.encode_utf16().flat_map(u16::to_le_bytes).collect(),
                    vtype: winreg::enums::REG_EXPAND_SZ,
                },
            )?;
            // Tell Explorer about the changed user environment so new applications
            // inherit PATH. Existing editors still need to be restarted.
            #[link(name = "user32")]
            unsafe extern "system" {
                fn SendMessageTimeoutW(
                    window: *mut std::ffi::c_void,
                    message: u32,
                    wparam: usize,
                    lparam: isize,
                    flags: u32,
                    timeout: u32,
                    result: *mut usize,
                ) -> isize;
            }
            let environment: Vec<u16> = "Environment\0".encode_utf16().collect();
            let mut result = 0;
            unsafe {
                SendMessageTimeoutW(
                    0xffff_usize as *mut std::ffi::c_void,
                    0x001a,
                    0,
                    environment.as_ptr() as isize,
                    0x0002,
                    5000,
                    &mut result,
                );
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        use std::io::Write;
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
        let path = bin.to_str().ok_or("Non-UTF-8 install directory")?;
        if path.contains(['\r', '\n']) {
            return Err("Install path cannot contain line breaks".into());
        }
        let line = format!("export PATH={}:\"$PATH\"", quote(path));
        let mut files = vec![home.join(".profile"), home.join(".bashrc")];
        #[cfg(target_os = "macos")]
        files.push(home.join(".zprofile"));
        if home.join(".bash_profile").exists() {
            files.push(home.join(".bash_profile"));
        }
        for file in files {
            let old = match fs::read_to_string(&file) {
                Ok(s) => s,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(e.into()),
            };
            if !old.lines().any(|s| s == line) {
                let mut out = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&file)?;
                writeln!(out, "\n# Chuchu compiler\n{line}")?;
            }
        }
    }
    Ok(())
}
