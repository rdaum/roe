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

//! Roe editor with Vello/GPU rendering backend.

use roe_core::Frame;
use roe_core::startup::{StartupArguments, StartupConfiguration, print_help};

const DEFAULT_COLS: u16 = 120;
const DEFAULT_LINES: u16 = 40;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let configuration = match StartupConfiguration::parse(std::env::args().skip(1)) {
        Ok(StartupArguments::Open(configuration)) => configuration,
        Ok(StartupArguments::Help) => {
            print_help("roe-vello");
            return Ok(());
        }
        Err(error) => {
            eprintln!("Error: {error}");
            print_help("roe-vello");
            std::process::exit(2);
        }
    };
    tracing::info!("starting Vello frontend");
    let runtime = compio::runtime::Runtime::new()?;
    let editor =
        runtime.block_on(configuration.create_editor(Frame::new(DEFAULT_COLS, DEFAULT_LINES)))?;
    roe_vello::run_vello_with_recovery(editor, runtime, configuration.recovery)?;
    tracing::info!("Vello frontend stopped");
    Ok(())
}
