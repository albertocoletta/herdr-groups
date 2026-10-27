mod daemon;
mod herdr;
mod layout;
mod model;
mod project;
mod sync;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use daemon::{Daemon, Toggle};
use herdr::Client;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

fn main() -> ExitCode {
    let Some(state_dir) = std::env::var_os("HERDR_PLUGIN_STATE_DIR").map(PathBuf::from) else {
        eprintln!("groups: run me as a herdr plugin (HERDR_PLUGIN_STATE_DIR is not set)");
        return ExitCode::FAILURE;
    };

    match std::env::args().nth(1).as_deref() {
        None | Some("start") => start(state_dir),
        Some("stop") => match daemon::request_stop(&state_dir, STOP_TIMEOUT) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => {
                eprintln!("groups: still running after {STOP_TIMEOUT:?}");
                ExitCode::FAILURE
            }
            Err(error) => {
                eprintln!("groups: cannot request stop: {error}");
                ExitCode::FAILURE
            }
        },
        Some("toggle") => match daemon::toggle(&state_dir, STOP_TIMEOUT) {
            Ok(Toggle::Start(lock)) => run(&state_dir, lock),
            Ok(Toggle::Stopped) => ExitCode::SUCCESS,
            Ok(Toggle::StillRunning) => {
                eprintln!("groups: still running after {STOP_TIMEOUT:?}");
                ExitCode::FAILURE
            }
            Err(error) => {
                eprintln!("groups: cannot toggle: {error}");
                ExitCode::FAILURE
            }
        },
        Some(other) => {
            eprintln!("groups: unknown command '{other}' (start | stop | toggle)");
            ExitCode::FAILURE
        }
    }
}

fn start(state_dir: PathBuf) -> ExitCode {
    match daemon::lock_for_start(&state_dir, STOP_TIMEOUT) {
        Ok(Some(lock)) => run(&state_dir, lock),
        Ok(None) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("groups: cannot lock {}: {error}", state_dir.display());
            ExitCode::FAILURE
        }
    }
}

fn run(state_dir: &Path, _lock: File) -> ExitCode {
    let Some(client) = Client::from_env() else {
        eprintln!("groups: HERDR_SOCKET_PATH is not set");
        return ExitCode::FAILURE;
    };
    let plugin_id = std::env::var("HERDR_PLUGIN_ID").unwrap_or_else(|_| "albertocoletta.groups".into());

    let exit = Daemon::new(client, format!("plugin:{plugin_id}"), state_dir).run(|| thread::sleep(POLL_INTERVAL));
    eprintln!("groups: exited ({exit:?})");
    ExitCode::SUCCESS
}
