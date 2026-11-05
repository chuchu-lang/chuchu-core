use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use inkwell::module::Module;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{CodeModel, InitializationConfig, RelocMode, Target, TargetMachine};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Optimization {
    O0,
    O1,
    #[default]
    O2,
    O3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidOptimization(pub String);
impl fmt::Display for InvalidOptimization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invalid optimization flag {:?}; expected -O0, -O1, -O2, or -O3",
            self.0
        )
    }
}
impl std::error::Error for InvalidOptimization {}
impl FromStr for Optimization {
    type Err = InvalidOptimization;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "-O0" => Ok(Self::O0),
            "-O1" => Ok(Self::O1),
            "-O2" => Ok(Self::O2),
            "-O3" => Ok(Self::O3),
            _ => Err(InvalidOptimization(value.to_owned())),
        }
    }
}
impl Optimization {
    pub fn pipeline(self) -> Option<&'static str> {
        match self {
            Self::O0 => None,
            Self::O1 => Some("default<O1>"),
            Self::O2 => Some("default<O2>"),
            Self::O3 => Some("default<O3>"),
        }
    }
    pub fn machine_level(self) -> inkwell::OptimizationLevel {
        match self {
            Self::O0 => inkwell::OptimizationLevel::None,
            Self::O1 => inkwell::OptimizationLevel::Less,
            Self::O2 => inkwell::OptimizationLevel::Default,
            Self::O3 => inkwell::OptimizationLevel::Aggressive,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Cpu {
    #[default]
    Generic,
    Native,
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub debug_info: DebugInfo,
    pub optimization: Optimization,
    pub cpu: Cpu,
    pub vectorize: bool,
    pub emit_ir: Option<PathBuf>,
    pub emit_assembly: Option<PathBuf>,
}
impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            debug_info: DebugInfo::None,
            optimization: Optimization::default(),
            cpu: Cpu::default(),
            vectorize: true,
            emit_ir: None,
            emit_assembly: None,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DebugInfo {
    #[default]
    None,
    Full,
}

#[derive(Debug)]
pub enum OptimizationError {
    Target(String),
    Verification(String),
    Pipeline(String),
}
impl fmt::Display for OptimizationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Target(message) => write!(f, "LLVM target error: {message}"),
            Self::Verification(message) => write!(f, "LLVM verification failed: {message}"),
            Self::Pipeline(message) => write!(f, "LLVM optimization failed: {message}"),
        }
    }
}
impl std::error::Error for OptimizationError {}

/// Configures target information before running any target-aware IR pipeline.
pub fn target_machine(
    module: &Module<'_>,
    options: &BuildOptions,
) -> Result<TargetMachine, OptimizationError> {
    if cfg!(target_os = "windows") && !cfg!(all(target_env = "msvc", target_arch = "x86_64")) {
        return Err(OptimizationError::Target(
            "Windows native support requires x86_64-pc-windows-msvc".to_owned(),
        ));
    }
    Target::initialize_native(&InitializationConfig::default())
        .map_err(OptimizationError::Target)?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple)
        .map_err(|error| OptimizationError::Target(error.to_string()))?;
    let (cpu, features) = match options.cpu {
        Cpu::Generic => ("generic".to_owned(), String::new()),
        Cpu::Native => (
            TargetMachine::get_host_cpu_name().to_string(),
            TargetMachine::get_host_cpu_features().to_string(),
        ),
    };
    let machine = target
        .create_target_machine(
            &triple,
            &cpu,
            &features,
            options.optimization.machine_level(),
            if cfg!(windows) {
                RelocMode::Default
            } else {
                RelocMode::PIC
            },
            CodeModel::Default,
        )
        .ok_or_else(|| OptimizationError::Target("Cannot create target machine".to_owned()))?;
    module.set_triple(&triple);
    let target_data = machine.get_target_data();
    let data_layout = target_data.get_data_layout();
    module.set_data_layout(&data_layout);
    #[cfg(target_os = "windows")]
    {
        std::mem::forget(data_layout);
        std::mem::forget(target_data);
        std::mem::forget(triple);
    }
    Ok(machine)
}

pub fn optimize(
    module: &Module<'_>,
    machine: &TargetMachine,
    options: &BuildOptions,
) -> Result<(), OptimizationError> {
    module
        .verify()
        .map_err(|error| OptimizationError::Verification(error.to_string()))?;
    if let Some(pipeline) = options.optimization.pipeline() {
        let passes = PassBuilderOptions::create();
        passes.set_verify_each(true);
        let vectorize = options.vectorize
            && matches!(options.optimization, Optimization::O2 | Optimization::O3);
        passes.set_loop_vectorization(vectorize);
        passes.set_loop_slp_vectorization(vectorize);
        passes.set_loop_interleaving(vectorize);
        module
            .run_passes(pipeline, machine, passes)
            .map_err(|error| OptimizationError::Pipeline(error.to_string()))?;
    }
    module
        .verify()
        .map_err(|error| OptimizationError::Verification(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_is_o2_generic() {
        assert_eq!(BuildOptions::default().optimization, Optimization::O2);
        assert_eq!(BuildOptions::default().cpu, Cpu::Generic);
    }
    #[test]
    fn flags_are_strict_and_o0_has_no_pipeline() {
        for (flag, level) in [
            ("-O0", Optimization::O0),
            ("-O1", Optimization::O1),
            ("-O2", Optimization::O2),
            ("-O3", Optimization::O3),
        ] {
            assert_eq!(flag.parse(), Ok(level));
        }
        assert!("-O9".parse::<Optimization>().is_err());
        assert!("O2".parse::<Optimization>().is_err());
        assert_eq!(Optimization::O0.pipeline(), None);
    }
}
