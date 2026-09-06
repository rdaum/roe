// Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free
// software: you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation, version
// 3.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along with
// this program. If not, see <https://www.gnu.org/licenses/>.
//

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::disable_raw_mode;
use roe_core::Frame;
use roe_core::session::{AttachmentConfiguration, LifecycleEvent, SessionClient};
use roe_core::startup::{StartupArguments, StartupConfiguration, print_help};
use roe_terminal::{ECHO_AREA_HEIGHT, TerminalRenderer};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

fn parse_args() -> StartupConfiguration {
    match StartupConfiguration::parse(std::env::args().skip(1)) {
        Ok(StartupArguments::Open(configuration)) => configuration,
        Ok(StartupArguments::Help) => {
            print_help("roe");
            std::process::exit(0);
        }
        Err(error) => {
            eprintln!("Error: {error}");
            print_help("roe");
            std::process::exit(2);
        }
    }
}

struct TerminalSession<W: Write> {
    device: W,
    active: bool,
}

impl<W: Write> TerminalSession<W> {
    fn enter(device: W) -> Result<Self, std::io::Error> {
        crossterm::terminal::enable_raw_mode()?;
        let mut session = Self {
            device,
            active: true,
        };
        execute!(
            session.device,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        execute!(session.device, crossterm::cursor::EnableBlinking)?;
        execute!(session.device, EnableMouseCapture)?;
        Ok(session)
    }

    fn device_mut(&mut self) -> &mut W {
        &mut self.device
    }

    fn cleanup(&mut self) -> Result<(), std::io::Error> {
        if !self.active {
            return Ok(());
        }
        exit_state(&mut self.device)?;
        self.active = false;
        Ok(())
    }
}

impl<W: Write> Drop for TerminalSession<W> {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("Warning: Failed to clean up terminal state: {error}");
        }
    }
}

fn install_signal_handlers(shutdown_requested: Arc<AtomicBool>) -> Result<(), std::io::Error> {
    signal_hook::flag::register(signal_hook::consts::SIGINT, shutdown_requested.clone())?;
    signal_hook::flag::register(signal_hook::consts::SIGTERM, shutdown_requested)?;
    Ok(())
}

// Everything to run in raw_mode
async fn terminal_main<W: Write>(
    stdout: W,
    config: StartupConfiguration,
    shutdown_requested: &AtomicBool,
) -> Result<(), std::io::Error> {
    assert!(crossterm::terminal::is_raw_mode_enabled()?);
    let _ws = crossterm::terminal::window_size()?;

    // Set the size of the screen
    assert_ne!(crossterm::terminal::size()?, (0, 0));

    let tsize = crossterm::terminal::size()?;

    let editor = config
        .create_editor(Frame::new(
            tsize.0,
            tsize.1.saturating_sub(ECHO_AREA_HEIGHT),
        ))
        .await?;
    let recovery = config.recovery;

    // Phase 5 keeps theme realization native; Mica face/configuration
    // relations can describe future theme changes without owning ANSI state.
    let theme = roe_terminal::terminal_renderer::CachedTheme::default();

    let mut renderer = TerminalRenderer::new_with_theme(stdout, theme);
    let attachment = AttachmentConfiguration::local_frontend(
        editor.frame().available_columns,
        editor.frame().available_lines,
    );
    let mut session = roe_core::startup::attach_editor(editor, attachment, &recovery, None)
        .await
        .map_err(std::io::Error::other)?;

    let mut frontend_services = roe_core::frontend::LocalFrontendServices::new();
    let initial = session.initial_output().await;
    let quit = roe_core::frontend::consume_output(
        &mut session,
        &mut frontend_services,
        &mut renderer,
        initial,
    )
    .await
    .map_err(std::io::Error::other)?;
    if quit {
        session
            .terminate_workspace()
            .await
            .map_err(std::io::Error::other)?;
        return Ok(());
    }

    let event_loop_result = roe_terminal::terminal_renderer::session_event_loop_with_renderer(
        &mut renderer,
        &mut session,
        shutdown_requested,
        &mut frontend_services,
    )
    .await;

    match session.terminate_workspace().await {
        Ok(output) => {
            for event in output.lifecycle {
                if let LifecycleEvent::Warning(error) = event {
                    tracing::warn!(%error, "editor shutdown warning");
                }
            }
        }
        Err(error) => tracing::warn!(%error, "session shutdown warning"),
    }
    event_loop_result?;

    Ok(())
}

fn exit_state(device: &mut impl Write) -> Result<(), std::io::Error> {
    exit_state_with(device, disable_raw_mode)
}

fn exit_state_with(
    device: &mut impl Write,
    restore_raw_mode: impl FnOnce() -> Result<(), std::io::Error>,
) -> Result<(), std::io::Error> {
    let mut first_error = None;
    let mut retain_error = |result: Result<(), std::io::Error>| {
        if let Err(error) = result
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    };

    // Every presentation reset is best effort. Raw-mode restoration must run
    // even if the output device has already failed.
    retain_error(execute!(device, DisableMouseCapture));
    retain_error(execute!(device, crossterm::cursor::Show));
    retain_error(execute!(
        device,
        crossterm::cursor::SetCursorStyle::DefaultUserShape
    ));
    retain_error(execute!(device, PopKeyboardEnhancementFlags));
    retain_error(device.flush());
    retain_error(restore_raw_mode());

    retain_error(execute!(
        device,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
    ));
    let (_, height) = crossterm::terminal::size().unwrap_or((80, 24));
    retain_error(execute!(device, crossterm::cursor::MoveTo(0, height)));
    retain_error(device.flush());

    first_error.map_or(Ok(()), Err)
}

fn main() -> Result<(), std::io::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    // Preserve panic diagnostics. TerminalSession performs cleanup while the
    // stack unwinds, including failures after partial terminal setup.
    std::panic::set_hook(Box::new(|panic_info| {
        eprintln!("💥 Roe has crashed! This shouldn't happen - please file a bug report at:");
        eprintln!("   https://github.com/rdaum/roe/issues");
        eprintln!();
        eprintln!("Include the following crash details in your report:");
        eprintln!("{panic_info}");
    }));

    // Parse command line arguments
    let config = parse_args();
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    install_signal_handlers(shutdown_requested.clone())?;

    tracing::info!("starting terminal frontend");
    let mut terminal = TerminalSession::enter(std::io::stdout())?;

    let result = compio::runtime::Runtime::new()?.block_on(terminal_main(
        terminal.device_mut(),
        config,
        shutdown_requested.as_ref(),
    ));

    terminal.cleanup()?;
    tracing::info!("terminal frontend stopped");

    // Handle the main result
    if let Err(e) = result {
        eprintln!("Error: {e}");
        return Err(e);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::exit_state_with;
    use std::cell::Cell;
    use std::io::{self, Write};

    struct FailedTerminal;

    impl Write for FailedTerminal {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal gone"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal gone"))
        }
    }

    #[test]
    fn raw_mode_restoration_runs_after_terminal_output_failure() {
        let restored = Cell::new(false);
        let error = exit_state_with(&mut FailedTerminal, || {
            restored.set(true);
            Ok(())
        })
        .expect_err("the original terminal output error must be retained");

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(restored.get());
    }
}
