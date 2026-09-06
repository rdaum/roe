// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Shared command-line input and native startup construction for both frontends.

use crate::buffer::BufferKind;
use crate::session::{
    AttachmentConfiguration, DirectSessionClient, MAX_SESSION_VIEWS, MAX_TEXT_CHARS_PER_INPUT,
    StartupRecoveryOperation, WorkspaceHost,
};
use crate::{Buffer, Editor, Frame};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("failed to start Mica host: {0}")]
    Mica(#[from] crate::mica_host::MicaHostError),
    #[error("Mica startup recovery failed: {0}")]
    Recovery(String),
}

/// Attach both local frontends through the same policy and recovery boundary.
pub async fn attach_editor(
    mut editor: Editor,
    attachment: AttachmentConfiguration,
    recovery: &[StartupRecoveryOperation],
    wake: Option<Arc<dyn crate::native_services::FrontendWake>>,
) -> Result<DirectSessionClient, StartupError> {
    if let Some(wake) = wake.as_ref() {
        editor.set_native_wake_handler(wake.clone());
    }
    let mut workspace = WorkspaceHost::open_with_mica(
        editor,
        crate::native_kernel::CapabilityGrants::editor_default(),
    )?;
    if let Some(wake) = wake {
        workspace.set_mica_wake_handler(wake);
    }
    let reports = match workspace.execute_startup_recovery(recovery).await {
        Ok(reports) => reports,
        Err(error) => {
            let mut attachment = workspace.attach(attachment);
            let _ = workspace.terminate_workspace(&mut attachment).await;
            return Err(StartupError::Recovery(error));
        }
    };
    if let Some(report) = reports.last() {
        workspace.set_recovery_message(report.clone());
    }
    Ok(DirectSessionClient::new(workspace, attachment))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StartupConfiguration {
    pub file_paths: Vec<PathBuf>,
    pub recovery: Vec<StartupRecoveryOperation>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartupArguments {
    Help,
    Open(StartupConfiguration),
}

impl StartupConfiguration {
    /// Parse arguments without the program name. No process exit or terminal access.
    pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<StartupArguments, String> {
        let mut arguments = arguments.into_iter();
        let mut configuration = Self::default();
        let mut paths_only = false;
        while let Some(argument) = arguments.next() {
            if argument.chars().count() > MAX_TEXT_CHARS_PER_INPUT {
                return Err("startup argument exceeds the text limit".into());
            }
            let mut operand = |description: &str| -> Result<String, String> {
                arguments
                    .next()
                    .filter(|value| {
                        !value.starts_with('-') && value.chars().count() <= MAX_TEXT_CHARS_PER_INPUT
                    })
                    .ok_or_else(|| {
                        format!("{argument} requires {description} within the text limit")
                    })
            };
            if paths_only {
                configuration.file_paths.push(argument.into());
            } else {
                match argument.as_str() {
                    "-h" | "--help" => return Ok(StartupArguments::Help),
                    "--" => paths_only = true,
                    "--mica-check" => configuration
                        .recovery
                        .push(StartupRecoveryOperation::CheckFile(operand("FILE")?.into())),
                    "--mica-replace" => {
                        configuration
                            .recovery
                            .push(StartupRecoveryOperation::ReplaceUnit {
                                unit: operand("UNIT FILE")?,
                                path: operand("UNIT FILE")?.into(),
                            })
                    }
                    "--mica-export" => {
                        configuration
                            .recovery
                            .push(StartupRecoveryOperation::ExportUnit {
                                unit: operand("UNIT FILE")?,
                                path: operand("UNIT FILE")?.into(),
                            })
                    }
                    "--mica-restore-first-wave" => configuration
                        .recovery
                        .push(StartupRecoveryOperation::RestoreFirstWave),
                    "--mica-enable-package" | "--mica-disable-package" => configuration
                        .recovery
                        .push(StartupRecoveryOperation::SetPackageEnabled {
                            package: operand("PACKAGE")?,
                            enabled: argument == "--mica-enable-package",
                        }),
                    "--mica-inspect" => configuration
                        .recovery
                        .push(StartupRecoveryOperation::Inspect),
                    option if option.starts_with('-') => {
                        return Err(format!("unknown option {option:?}"));
                    }
                    _ => configuration.file_paths.push(argument.into()),
                }
            }
            if configuration.file_paths.len() > MAX_SESSION_VIEWS
                || configuration.recovery.len() > MAX_SESSION_VIEWS
            {
                return Err(format!(
                    "startup accepts at most {MAX_SESSION_VIEWS} files and recovery operations"
                ));
            }
        }
        Ok(StartupArguments::Open(configuration))
    }

    pub async fn create_editor(&self, frame: Frame) -> std::io::Result<Editor> {
        if self.file_paths.len() > MAX_SESSION_VIEWS {
            return Err(std::io::Error::other("startup file limit exceeded"));
        }
        let mut editor = Editor::new(Buffer::named("*scratch*", BufferKind::Scratch), frame);
        if self.file_paths.is_empty() {
            let buffer = Buffer::named("*Welcome*", BufferKind::Internal);
            buffer.load_str(&welcome_content());
            buffer.set_read_only(true);
            editor.startup_buffers.push(editor.buffers.insert(buffer));
        } else {
            for path in &self.file_paths {
                let buffer = match Buffer::from_file(path).await {
                    Ok(buffer) => buffer,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        Buffer::visiting(path.clone())
                    }
                    Err(error) => {
                        return Err(std::io::Error::new(
                            error.kind(),
                            format!("failed to open {}: {error}", path.display()),
                        ));
                    }
                };
                editor.startup_buffers.push(editor.buffers.insert(buffer));
            }
        }
        if let Err(error) = editor.file_watcher.init() {
            editor.set_echo_message(format!("file watcher is unavailable: {error}"));
        }
        for id in editor.startup_buffers.clone() {
            let buffer = &editor.buffers[id];
            if let Some(path) = buffer.visited_file()
                && path.exists()
                && let Err(error) = editor.file_watcher.watch_file(id, &path, buffer.content())
            {
                editor.set_echo_message(format!("failed to watch {}: {error}", path.display()));
            }
        }
        Ok(editor)
    }
}

pub fn print_help(program: &str) {
    println!("Roe - Ryan's Own Emacs\n\nUSAGE:\n    {program} [OPTIONS] [FILES...]\n");
    println!(
        "OPTIONS:\n    -h, --help\n    --mica-check FILE\n    --mica-replace UNIT FILE\n    --mica-export UNIT FILE\n    --mica-restore-first-wave\n    --mica-enable-package PACKAGE\n    --mica-disable-package PACKAGE\n    --mica-inspect\n    --  Treat remaining arguments as file paths"
    );
}

fn welcome_content() -> String {
    format!(
        "{}\n\n                    ROE - Ryan's Own Emacs\n\n                     C-x C-f  -  Find and open a file\n                     C-x C-s  -  Save current buffer\n                     C-x C-c  -  Exit Roe\n                     M-x      -  Execute command\n                     C-x b    -  Switch buffer\n                     C-x 2    -  Split window horizontally\n                     C-x 3    -  Split window vertically\n                     C-x o    -  Switch to other window\n",
        include_str!("../../rune.txt")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_share_recovery_order_and_support_literal_paths() {
        let parsed = StartupConfiguration::parse(
            [
                "--mica-check",
                "a.mica",
                "--mica-replace",
                "roe/user",
                "b.mica",
                "--",
                "-file",
            ]
            .map(str::to_owned),
        )
        .unwrap();
        let StartupArguments::Open(configuration) = parsed else {
            panic!("expected configuration")
        };
        assert_eq!(configuration.file_paths, [PathBuf::from("-file")]);
        assert_eq!(
            configuration.recovery,
            [
                StartupRecoveryOperation::CheckFile("a.mica".into()),
                StartupRecoveryOperation::ReplaceUnit {
                    unit: "roe/user".into(),
                    path: "b.mica".into()
                }
            ]
        );
    }

    #[test]
    fn malformed_and_oversized_arguments_are_rejected() {
        for arguments in [
            vec!["--unknown"],
            vec!["--mica-check"],
            vec!["--mica-replace", "unit"],
            vec!["--mica-export", "--help"],
        ] {
            assert!(StartupConfiguration::parse(arguments.into_iter().map(str::to_owned)).is_err());
        }
        assert!(
            StartupConfiguration::parse(std::iter::repeat_n(
                "file".to_owned(),
                MAX_SESSION_VIEWS + 1
            ))
            .is_err()
        );
    }
}
