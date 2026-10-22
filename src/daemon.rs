use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::herdr::{Error, Herdr};
use crate::layout::layout;
use crate::model;
use crate::project::Projects;
use crate::sync::Sync;

const LOCK_FILE: &str = "daemon.lock";
const STOP_FILE: &str = "stop";
const SOCKET_FAILURES_BEFORE_EXIT: u32 = 3;
const STOP_POLL: Duration = Duration::from_millis(50);

pub fn lock(state_dir: &Path) -> io::Result<Option<File>> {
    fs::create_dir_all(state_dir)?;
    let file = File::options().create(true).truncate(false).write(true).open(state_dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

pub fn request_stop(state_dir: &Path, timeout: Duration) -> io::Result<bool> {
    write_stop_request(state_dir)?;
    let deadline = Instant::now() + timeout;
    loop {
        if lock(state_dir)?.is_some() {
            let _ = fs::remove_file(state_dir.join(STOP_FILE));
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(STOP_POLL);
    }
}

fn write_stop_request(state_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(state_dir)?;
    File::create(state_dir.join(STOP_FILE)).map(|_| ())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Exit {
    Stopped,
    SocketGone,
    ViewRejected,
}

pub struct Daemon<H: Herdr> {
    herdr: H,
    source: String,
    stop_file: PathBuf,
    projects: Projects,
    sync: Sync,
}

impl<H: Herdr> Daemon<H> {
    pub fn new(herdr: H, source: String, state_dir: &Path) -> Self {
        Self {
            herdr,
            sync: Sync::new(source.clone()),
            source,
            stop_file: state_dir.join(STOP_FILE),
            projects: Projects::new(),
        }
    }

    pub fn run(&mut self, mut wait: impl FnMut()) -> Exit {
        let _ = fs::remove_file(&self.stop_file);
        if let Err(error) = self.set_view() {
            eprintln!("groups: agent view rejected: {error}");
            return Exit::ViewRejected;
        }

        let mut socket_failures = 0;
        loop {
            if self.stop_file.exists() {
                let _ = fs::remove_file(&self.stop_file);
                self.sync.clear_all(&self.herdr);
                let _ = self.herdr.call("agent.view.clear", json!({ "source": self.source }));
                return Exit::Stopped;
            }
            match self.tick() {
                Ok(()) => socket_failures = 0,
                Err(Error::Io(error)) => {
                    socket_failures += 1;
                    if socket_failures >= SOCKET_FAILURES_BEFORE_EXIT {
                        eprintln!("groups: socket gone: {error}");
                        return Exit::SocketGone;
                    }
                }
                Err(error) => eprintln!("groups: {error}"),
            }
            wait();
        }
    }

    fn set_view(&self) -> Result<Value, Error> {
        self.herdr.call(
            "agent.view.set",
            json!({
                "source": self.source,
                "label": "groups",
                "sort": [{ "field": { "token": "order" }, "order": "asc" }],
            }),
        )
    }

    fn tick(&mut self) -> Result<(), Error> {
        let mut result = self.herdr.call("session.snapshot", json!({}))?;
        let snapshot = result.get_mut("snapshot").map(Value::take).unwrap_or(Value::Null);
        let agents = model::agents(snapshot).map_err(|error| Error::Protocol(error.to_string()))?;
        let rows = layout(&agents, |cwd| self.projects.name_for(cwd));
        for error in self.sync.apply(&self.herdr, rows) {
            eprintln!("groups: {error}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tempfile::TempDir;

    #[derive(Clone, Default)]
    struct FakeHerdr {
        calls: Rc<RefCell<Vec<String>>>,
        socket_gone: bool,
        reject_view: bool,
    }

    impl FakeHerdr {
        fn count(&self, method: &str) -> usize {
            self.calls.borrow().iter().filter(|call| *call == method).count()
        }
    }

    impl Herdr for FakeHerdr {
        fn call(&self, method: &str, _params: Value) -> Result<Value, Error> {
            self.calls.borrow_mut().push(method.to_string());
            match method {
                "agent.view.set" if self.reject_view => {
                    Err(Error::Herdr { code: "plugin_disabled".into(), message: String::new() })
                }
                "session.snapshot" if self.socket_gone => {
                    Err(Error::Io(io::Error::from(io::ErrorKind::ConnectionRefused)))
                }
                "session.snapshot" => Ok(json!({ "snapshot": {
                    "agents": [{
                        "pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1",
                        "cwd": "/nonexistent/herdr-groups-test/api", "terminal_title_stripped": "Claude Code"
                    }],
                    "workspaces": [{ "workspace_id": "w1" }],
                    "tabs": [{ "tab_id": "w1:t1" }],
                    "panes": [{ "pane_id": "w1:p1" }]
                }})),
                _ => Ok(json!({})),
            }
        }
    }

    fn stop_after(state_dir: &Path, ticks: usize) -> impl FnMut() {
        let state_dir = state_dir.to_path_buf();
        let mut waited = 0;
        move || {
            waited += 1;
            if waited == ticks {
                write_stop_request(&state_dir).unwrap();
            }
        }
    }

    #[test]
    fn second_instance_cannot_lock() {
        let dir = TempDir::new().unwrap();
        let _first = lock(dir.path()).unwrap().unwrap();
        assert!(lock(dir.path()).unwrap().is_none());
    }

    #[test]
    fn lock_is_released_when_dropped() {
        let dir = TempDir::new().unwrap();
        drop(lock(dir.path()).unwrap().unwrap());
        assert!(lock(dir.path()).unwrap().is_some());
    }

    #[test]
    fn stop_waits_until_the_running_instance_releases_its_lock() {
        let dir = TempDir::new().unwrap();
        let held = lock(dir.path()).unwrap().unwrap();
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            drop(held);
        });
        let started = Instant::now();
        assert!(request_stop(dir.path(), Duration::from_secs(3)).unwrap());
        assert!(started.elapsed() >= Duration::from_millis(200));
        releaser.join().unwrap();
    }

    #[test]
    fn stop_gives_up_after_the_timeout() {
        let dir = TempDir::new().unwrap();
        let _held = lock(dir.path()).unwrap().unwrap();
        assert!(!request_stop(dir.path(), Duration::from_millis(200)).unwrap());
    }

    #[test]
    fn stop_without_a_running_instance_leaves_no_stop_request() {
        let dir = TempDir::new().unwrap();
        assert!(request_stop(dir.path(), Duration::from_secs(3)).unwrap());
        assert!(!dir.path().join(STOP_FILE).exists());
    }

    #[test]
    fn closed_socket_ends_the_loop() {
        let dir = TempDir::new().unwrap();
        let herdr = FakeHerdr { socket_gone: true, ..FakeHerdr::default() };
        let exit = Daemon::new(herdr.clone(), "plugin:test".into(), dir.path()).run(|| {});
        assert_eq!(exit, Exit::SocketGone);
        assert_eq!(herdr.count("session.snapshot"), 3);
    }

    #[test]
    fn unchanged_snapshot_reports_once() {
        let dir = TempDir::new().unwrap();
        let herdr = FakeHerdr::default();
        let exit = Daemon::new(herdr.clone(), "plugin:test".into(), dir.path()).run(stop_after(dir.path(), 3));
        assert_eq!(exit, Exit::Stopped);
        assert_eq!(herdr.count("session.snapshot"), 3);
        assert_eq!(herdr.count("pane.report_metadata"), 2);
    }

    #[test]
    fn stop_clears_tokens_and_view() {
        let dir = TempDir::new().unwrap();
        let herdr = FakeHerdr::default();
        Daemon::new(herdr.clone(), "plugin:test".into(), dir.path()).run(stop_after(dir.path(), 1));
        assert_eq!(herdr.calls.borrow().last().unwrap(), "agent.view.clear");
        assert!(!dir.path().join(STOP_FILE).exists());
    }

    #[test]
    fn stale_stop_request_is_ignored_at_start() {
        let dir = TempDir::new().unwrap();
        write_stop_request(dir.path()).unwrap();
        let herdr = FakeHerdr::default();
        Daemon::new(herdr.clone(), "plugin:test".into(), dir.path()).run(stop_after(dir.path(), 1));
        assert_eq!(herdr.count("session.snapshot"), 1);
    }

    #[test]
    fn rejected_view_exits_without_polling() {
        let dir = TempDir::new().unwrap();
        let herdr = FakeHerdr { reject_view: true, ..FakeHerdr::default() };
        let exit = Daemon::new(herdr.clone(), "plugin:test".into(), dir.path()).run(|| {});
        assert_eq!(exit, Exit::ViewRejected);
        assert_eq!(herdr.count("session.snapshot"), 0);
    }
}
