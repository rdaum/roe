// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Recovery operations only receive the Mica host. No editor, kernel, or attachment access.

use super::protocol::{
    LifecycleEvent, MAX_SESSION_VIEWS, RecoveryOperation, StartupRecoveryOperation,
};
use crate::mica_host::MicaHost;
use crate::native_io::{MAX_IO_BYTES, read_text, write_text};

pub(super) async fn startup(
    mica: Option<&mut MicaHost>,
    operations: &[StartupRecoveryOperation],
) -> Result<Vec<String>, String> {
    if operations.len() > MAX_SESSION_VIEWS {
        return Err("startup recovery operation limit exceeded".into());
    }
    if operations.is_empty() {
        return Ok(Vec::new());
    }
    let mica = mica.ok_or_else(|| "Mica recovery host is unavailable".to_owned())?;
    let mut reports = Vec::new();
    for operation in operations {
        match operation {
            StartupRecoveryOperation::CheckFile(path) => {
                let source = read_text(path, MAX_IO_BYTES)
                    .await
                    .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
                mica.check_source(source).await.map_err(|error| {
                    format!("Mica check failed for {}: {error}", path.display())
                })?;
                reports.push(format!("Mica source check passed: {}", path.display()));
            }
            StartupRecoveryOperation::ReplaceUnit { unit, path } => {
                let source = read_text(path, MAX_IO_BYTES)
                    .await
                    .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
                mica.replace_unit(unit, source)
                    .await
                    .map_err(|error| format!("Mica replacement of {unit} failed: {error}"))?;
                reports.push(format!("Replaced Mica unit {unit} from {}", path.display()));
            }
            StartupRecoveryOperation::ExportUnit { unit, path } => {
                let source = mica
                    .export_unit(unit)
                    .await
                    .map_err(|error| format!("Mica export of {unit} failed: {error}"))?;
                write_text(path, source, MAX_IO_BYTES)
                    .await
                    .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
                reports.push(format!("Exported Mica unit {unit} to {}", path.display()));
            }
            StartupRecoveryOperation::RestoreFirstWave => {
                mica.restore_first_wave()
                    .await
                    .map_err(|error| format!("Mica first-wave restore failed: {error}"))?;
                reports.push("Restored the built-in Mica first wave".to_owned());
            }
            StartupRecoveryOperation::SetPackageEnabled { package, enabled } => {
                mica.set_package_enabled(package, *enabled)
                    .map_err(|error| format!("Mica package update failed: {error}"))?;
                reports.push(format!(
                    "Mica package {package} {}",
                    if *enabled { "enabled" } else { "disabled" }
                ));
            }
            StartupRecoveryOperation::Inspect => {
                let diagnostics = mica.recovery_diagnostics();
                reports.push(format!("Mica recovery diagnostics: {diagnostics}"));
            }
        }
    }
    Ok(reports)
}

pub(super) async fn dispatch(
    mica: Option<&mut MicaHost>,
    operation: RecoveryOperation,
) -> LifecycleEvent {
    let Some(mica) = mica else {
        return LifecycleEvent::RecoveryResult {
            operation: match operation {
                RecoveryOperation::CheckSource { .. } => "check-source",
                RecoveryOperation::ReplaceUnit { .. } => "replace-unit",
                RecoveryOperation::ExportUnit { .. } => "export-unit",
                RecoveryOperation::RestoreFirstWave => "restore-first-wave",
                RecoveryOperation::SetPackageEnabled { .. } => "set-package-enabled",
                RecoveryOperation::Inspect => "inspect",
            }
            .into(),
            result: Err("Mica recovery host is unavailable".into()),
        };
    };
    let (name, result) = match operation {
        RecoveryOperation::CheckSource { source } => (
            "check-source",
            mica.check_source(source)
                .await
                .map(|()| None)
                .map_err(|error| error.to_string()),
        ),
        RecoveryOperation::ReplaceUnit { unit, source } => (
            "replace-unit",
            mica.replace_unit(&unit, source)
                .await
                .map(|()| None)
                .map_err(|error| error.to_string()),
        ),
        RecoveryOperation::ExportUnit { unit } => (
            "export-unit",
            mica.export_unit(&unit)
                .await
                .map(Some)
                .map_err(|error| error.to_string()),
        ),
        RecoveryOperation::RestoreFirstWave => (
            "restore-first-wave",
            mica.restore_first_wave()
                .await
                .map(|()| None)
                .map_err(|error| error.to_string()),
        ),
        RecoveryOperation::SetPackageEnabled { package, enabled } => (
            "set-package-enabled",
            mica.set_package_enabled(&package, enabled)
                .map(|()| None)
                .map_err(|error| error.to_string()),
        ),
        RecoveryOperation::Inspect => ("inspect", Ok(Some(mica.recovery_diagnostics()))),
    };
    LifecycleEvent::RecoveryResult {
        operation: name.to_owned(),
        result,
    }
}
