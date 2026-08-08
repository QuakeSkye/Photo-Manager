mod cache_suite;
mod candidate_binding;
mod database_suite;
mod metrics;
mod performance;
mod safety_suite;
mod scale;
mod ui_fixture;

use std::{env, path::Path};

use serde::Serialize;

use crate::{
    domain::{DomainError, ErrorCode},
    safety::PhaseEightRunRoot,
};

pub use scale::ScaleReport;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatedRun {
    run_id: uuid::Uuid,
    run_root: String,
    media: crate::safety::PhaseEightMediaProfile,
    allocated_bytes: u64,
}

pub fn try_run_cli() -> Option<i32> {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.first().and_then(|value| value.to_str()) != Some("--phase8-self-test") {
        return None;
    }
    let result = run_cli(&arguments[1..]);
    match result {
        Ok(value) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&value).expect("serialize phase 8 result")
            );
            Some(0)
        }
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&error).expect("serialize phase 8 error")
            );
            Some(2)
        }
    }
}

fn run_cli(arguments: &[std::ffi::OsString]) -> Result<serde_json::Value, DomainError> {
    match arguments.first().and_then(|value| value.to_str()) {
        Some("create-run") if arguments.len() == 1 => {
            let root = PhaseEightRunRoot::create()?;
            json_value(CreatedRun {
                run_id: root.run_id(),
                run_root: root.path().display().to_string(),
                media: root.media_profile()?,
                allocated_bytes: root.allocated_bytes()?,
            })
        }
        Some("scale") => {
            let run_root = required_option(arguments, "--run-root")?;
            let output = required_option(arguments, "--output")?;
            let root = PhaseEightRunRoot::verify(Path::new(run_root))?;
            let report = scale::run(&root)?;
            let output_path = root.resolve_owned(Path::new(output))?;
            let bytes = serde_json::to_vec_pretty(&report).map_err(internal_error)?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            use std::io::Write;
            options
                .open(&output_path)
                .and_then(|mut file| file.write_all(&bytes).and_then(|()| file.sync_all()))
                .map_err(|error| {
                    DomainError::new(
                        ErrorCode::CapabilityDenied,
                        "无法以 create-new 写入阶段 8 报告。",
                    )
                    .with_diagnostic(error.to_string())
                })?;
            json_value(report)
        }
        Some("io") => {
            let run_root = required_option(arguments, "--run-root")?;
            let output = required_option(arguments, "--output")?;
            let root = PhaseEightRunRoot::verify(Path::new(run_root))?;
            let report = metrics::run(&root)?;
            write_report(&root, output, &report)?;
            json_value(report)
        }
        Some("safety") => {
            let run_root = required_option(arguments, "--run-root")?;
            let output = required_option(arguments, "--output")?;
            let root = PhaseEightRunRoot::verify(Path::new(run_root))?;
            let report = safety_suite::run(&root)?;
            write_report(&root, output, &report)?;
            json_value(report)
        }
        Some("database") => {
            let run_root = required_option(arguments, "--run-root")?;
            let output = required_option(arguments, "--output")?;
            let root = PhaseEightRunRoot::verify(Path::new(run_root))?;
            let report = database_suite::run(&root)?;
            write_report(&root, output, &report)?;
            json_value(report)
        }
        Some("cache") => {
            let run_root = required_option(arguments, "--run-root")?;
            let output = required_option(arguments, "--output")?;
            let root = PhaseEightRunRoot::verify(Path::new(run_root))?;
            let report = cache_suite::run(&root)?;
            write_report(&root, output, &report)?;
            json_value(report)
        }
        Some("performance") => {
            let run_root = required_option(arguments, "--run-root")?;
            let output = required_option(arguments, "--output")?;
            let root = PhaseEightRunRoot::verify(Path::new(run_root))?;
            let report = performance::run(&root)?;
            write_report(&root, output, &report)?;
            json_value(report)
        }
        Some("prepare-ui") if arguments.len() == 3 => {
            let run_root = required_option(arguments, "--run-root")?;
            let root = PhaseEightRunRoot::verify(Path::new(run_root))?;
            json_value(ui_fixture::prepare(&root)?)
        }
        Some("candidate-manifest") => {
            let run_root = required_option(arguments, "--run-root")?;
            let installer = required_option(arguments, "--installer")?;
            let output = required_option(arguments, "--output")?;
            let root = PhaseEightRunRoot::verify(Path::new(run_root))?;
            let report = candidate_binding::capture(&root, Path::new(installer))?;
            write_report(&root, output, &report)?;
            json_value(report)
        }
        Some("create-admission") => {
            let run_root = required_option(arguments, "--run-root")?;
            let manifest = required_option(arguments, "--manifest")?;
            let feature = match required_option(arguments, "--feature")? {
                "import" => crate::safety::FeatureGate::Import,
                "recycle" => crate::safety::FeatureGate::Recycle,
                _ => return Err(invalid_cli()),
            };
            let safety_report = required_option(arguments, "--safety-report")?;
            let output = required_option(arguments, "--output")?;
            let evidence = repeated_options(arguments, "--evidence")?
                .into_iter()
                .map(Path::new)
                .collect::<Vec<_>>();
            let root = PhaseEightRunRoot::verify(Path::new(run_root))?;
            let admission = candidate_binding::capture_admission(
                &root,
                Path::new(manifest),
                feature,
                Path::new(safety_report),
                &evidence,
            )?;
            write_report(&root, output, &admission)?;
            json_value(admission)
        }
        Some("verify-candidate") => {
            let run_root = required_option(arguments, "--run-root")?;
            let manifest = required_option(arguments, "--manifest")?;
            let output = required_option(arguments, "--output")?;
            let root = PhaseEightRunRoot::verify(Path::new(run_root))?;
            let report = candidate_binding::verify(&root, Path::new(manifest))?;
            write_report(&root, output, &report)?;
            json_value(report)
        }
        Some("verify-admissions") => {
            let run_root = required_option(arguments, "--run-root")?;
            let manifest = required_option(arguments, "--manifest")?;
            let import = required_option(arguments, "--import")?;
            let recycle = required_option(arguments, "--recycle")?;
            let output = required_option(arguments, "--output")?;
            let root = PhaseEightRunRoot::verify(Path::new(run_root))?;
            let report = candidate_binding::verify_admissions(
                &root,
                Path::new(manifest),
                Path::new(import),
                Path::new(recycle),
            )?;
            write_report(&root, output, &report)?;
            json_value(report)
        }
        _ => Err(DomainError::new(
            ErrorCode::CapabilityDenied,
            "阶段 8 自测参数无效；仅支持 create-run 或 scale。",
        )),
    }
}

fn write_report(
    root: &PhaseEightRunRoot,
    output: &str,
    report: &impl Serialize,
) -> Result<(), DomainError> {
    let output_path = root.resolve_owned(Path::new(output))?;
    let bytes = serde_json::to_vec_pretty(report).map_err(internal_error)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    use std::io::Write;
    options
        .open(&output_path)
        .and_then(|mut file| file.write_all(&bytes).and_then(|()| file.sync_all()))
        .map_err(|error| {
            DomainError::new(
                ErrorCode::CapabilityDenied,
                "无法以 create-new 写入阶段 8 报告。",
            )
            .with_diagnostic(error.to_string())
        })
}

fn required_option<'a>(
    arguments: &'a [std::ffi::OsString],
    name: &str,
) -> Result<&'a str, DomainError> {
    let position = arguments
        .iter()
        .position(|value| value == name)
        .ok_or_else(invalid_cli)?;
    arguments
        .get(position + 1)
        .and_then(|value| value.to_str())
        .ok_or_else(invalid_cli)
}

fn repeated_options<'a>(
    arguments: &'a [std::ffi::OsString],
    name: &str,
) -> Result<Vec<&'a str>, DomainError> {
    let mut values = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        if argument == name {
            values.push(
                arguments
                    .get(index + 1)
                    .and_then(|value| value.to_str())
                    .ok_or_else(invalid_cli)?,
            );
        }
    }
    if values.is_empty() {
        return Err(invalid_cli());
    }
    Ok(values)
}

fn invalid_cli() -> DomainError {
    DomainError::new(ErrorCode::CapabilityDenied, "阶段 8 自测缺少必需参数。")
}

fn json_value(value: impl Serialize) -> Result<serde_json::Value, DomainError> {
    serde_json::to_value(value).map_err(internal_error)
}

fn internal_error(error: impl std::fmt::Display) -> DomainError {
    DomainError::new(ErrorCode::Internal, "阶段 8 自测报告生成失败。")
        .with_diagnostic(error.to_string())
}
