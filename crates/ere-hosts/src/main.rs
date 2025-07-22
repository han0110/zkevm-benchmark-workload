//! Binary for benchmarking different Ere compatible zkVMs

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

#[cfg(not(any(
    feature = "sp1",
    feature = "risc0",
    feature = "openvm",
    feature = "pico",
    feature = "zisk"
)))]
compile_error!("please enable one of the zkVM's using the appropriate feature flag");

use benchmark_runner::{Action, RunConfig, run_benchmark};
use clap::{Parser, Subcommand, ValueEnum};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use zkvm_interface::ProverResourceType;

#[cfg(feature = "sp1")]
use ere_sp1::EreSP1;

#[cfg(feature = "risc0")]
use ere_risczero::EreRisc0;

#[cfg(feature = "openvm")]
use ere_openvm::EreOpenVM;

#[cfg(feature = "pico")]
use ere_pico::ErePico;

#[cfg(feature = "zisk")]
use ere_zisk::EreZisk;

#[derive(Parser)]
#[command(name = "zkvm-benchmarker")]
#[command(about = "Benchmark different Ere compatible zkVMs")]
#[command(version)]
struct Cli {
    /// Resource type for proving
    #[arg(short, long, value_enum, default_value = "cpu")]
    resource: Resource,

    /// Action to perform
    #[arg(short, long, value_enum, default_value = "execute")]
    action: BenchmarkAction,

    /// Rerun the benchmarks even if the output folder already contains results
    #[arg(long, default_value_t = false)]
    force_rerun: bool,

    /// Guest program to benchmark
    #[command(subcommand)]
    guest_program: GuestProgramCommand,

    /// Output folder for benchmark results
    #[arg(short, long, default_value = "zkevm-metrics")]
    output_folder: PathBuf,
}

#[derive(Subcommand, Clone, Debug)]
enum GuestProgramCommand {
    /// Ethereum Stateless Validator
    StatelessValidator {
        /// Input folder for benchmark results
        #[arg(short, long, default_value = "zkevm-fixtures-input")]
        input_folder: PathBuf,
    },
    /// Empty program
    EmptyProgram,

    /// Block RLP length calculator
    RlpEncodingLength {
        /// Input folder for benchmark results
        #[arg(short, long, default_value = "zkevm-fixtures-input")]
        input_folder: PathBuf,

        /// Number of times to loop the benchmark
        #[arg(long)]
        loop_count: u16,
    },
}

#[derive(Clone, ValueEnum)]
enum Resource {
    Cpu,
    Gpu,
}

#[derive(Clone, ValueEnum)]
enum BenchmarkAction {
    Execute,
    Prove,
}

impl From<Resource> for ProverResourceType {
    fn from(resource: Resource) -> Self {
        match resource {
            Resource::Cpu => Self::Cpu,
            Resource::Gpu => Self::Gpu,
        }
    }
}

impl From<BenchmarkAction> for Action {
    fn from(action: BenchmarkAction) -> Self {
        match action {
            BenchmarkAction::Execute => Self::Execute,
            BenchmarkAction::Prove => Self::Prove,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    let resource: ProverResourceType = cli.resource.into();
    let action: Action = cli.action.into();
    info!(
        "Running benchmarks with resource={:?} and action={:?}",
        resource, action
    );

    let config = RunConfig {
        output_folder: cli.output_folder,
        action,
        force_rerun: cli.force_rerun,
    };

    let workspace_dir = workspace_root().join("ere-guests");
    match &cli.guest_program {
        GuestProgramCommand::StatelessValidator { input_folder } => {
            info!(
                "Running stateless-validator benchmark for input folder: {}",
                input_folder.display()
            );
            let inputs = benchmark_runner::guest_programs::stateless_validator_generate_inputs(
                input_folder.as_path(),
            )?;
            for_each_zkvm_do!(&workspace_dir, "stateless-validator", resource, |zkvm| {
                run_benchmark(&zkvm, &config, inputs.clone())?
            });
        }
        GuestProgramCommand::EmptyProgram => {
            info!("Running empty-program benchmarks");
            let input = benchmark_runner::guest_programs::empty_program_generate_inputs();
            for_each_zkvm_do!(&workspace_dir, "empty-program", resource, |zkvm| {
                run_benchmark(&zkvm, &config, vec![input.clone()])?
            });
        }
        GuestProgramCommand::RlpEncodingLength {
            input_folder,
            loop_count,
        } => {
            info!(
                "Running rlp-encoding-length benchmarks for input folder {} and loop count {}",
                input_folder.display(),
                loop_count
            );
            let inputs = benchmark_runner::guest_programs::block_rlp_length_generate_inputs(
                input_folder.as_path(),
                *loop_count,
            )?;
            for_each_zkvm_do!(&workspace_dir, "rlp-encoding-length", resource, |zkvm| {
                run_benchmark(&zkvm, &config, inputs.clone())?
            });
        }
    }

    Ok(())
}

macro_rules! for_each_zkvm_do {
    (@ $name:expr, $impl:ty, $workspace_dir:expr, $guest_relative:expr, $resource:expr, |$zkvm:ident| $do:expr) => {{
        run_cargo_patch_command($name, $workspace_dir)?;
        let program = <<$impl as zkvm_interface::zkVM>::Compiler as zkvm_interface::Compiler>::compile(
            $workspace_dir,
            &PathBuf::from($guest_relative).join($name),
        )?;
        let $zkvm = <$impl as zkvm_interface::zkVM>::new(program, $resource.clone())?;
        $do;
    }};
    ($workspace_dir:expr, $guest_relative:expr, $resource:expr, |$zkvm:ident| $do:expr) => {
        #[cfg(feature = "sp1")]
        for_each_zkvm_do!(@ "sp1", EreSP1, $workspace_dir, $guest_relative, $resource, |$zkvm| $do);

        #[cfg(feature = "zisk")]
        for_each_zkvm_do!(@ "zisk", EreZisk, $workspace_dir, $guest_relative, $resource, |$zkvm| $do);

        #[cfg(feature = "risc0")]
        for_each_zkvm_do!(@ "risc0", EreRisc0, $workspace_dir, $guest_relative, $resource, |$zkvm| $do);

        #[cfg(feature = "openvm")]
        for_each_zkvm_do!(@ "openvm", EreOpenVM, $workspace_dir, $guest_relative, $resource, |$zkvm| $do);

        #[cfg(feature = "pico")]
        for_each_zkvm_do!(@ "pico", ErePico, $workspace_dir, $guest_relative, $resource, |$zkvm| $do);
    };
}

use for_each_zkvm_do;

/// Patches the precompiles for a specific zkvm
fn run_cargo_patch_command(
    zkvm_name: &str,
    workspace_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Running cargo {}...", zkvm_name);

    let output = Command::new("cargo")
        .arg(zkvm_name)
        .arg("--manifest-folder")
        .arg(workspace_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);

        error!(
            "cargo {} failed with exit code: {:?}",
            zkvm_name,
            output.status.code()
        );
        error!("stdout: {}", stdout);
        error!("stderr: {}", stderr);

        return Err(format!("cargo {zkvm_name} command failed").into());
    }

    info!("cargo {zkvm_name} completed successfully");
    Ok(())
}

/// Repository root (assumes `ere-hosts` lives in `<root>/crates/ere-hosts`).
fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}
