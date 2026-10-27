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
const OFF_FILE: &str = "off";
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

pub fn lock_for_start(state_dir: &Path, timeout: Duration) -> io::Result<Option<File>> {
    if state_dir.join(OFF_FILE).exists() {
        return Ok(None);
    }
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(file) = lock(state_dir)? {
            return Ok(Some(file));
        }
        if !state_dir.join(STOP_FILE).exists() || Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(STOP_POLL);
    }
}

pub fn request_stop(state_dir: &Path, timeout: Duration) -> io::Result<bool> {
    write_stop_request(state_dir)?;
    let stop_file = state_dir.join(STOP_FILE);
    let deadline = Instant::now() + timeout;
    loop {
        if !stop_file.exists() {
            return Ok(true);
        }
        if let Some(released) = lock(state_dir)? {
            drop(released);
            let _ = fs::remove_file(&stop_file);
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(STOP_POLL);
    }
}

#[derive(Debug)]
pub enum Toggle {
    Start(File),
    Stopped,
    StillRunning,
}

pub fn toggle(state_dir: &Path, timeout: Duration) -> io::Result<Toggle> {
    let off_file = state_dir.join(OFF_FILE);
    if let Some(lock) = lock(state_dir)? {
        if off_file.exists() {
            fs::remove_file(&off_file)?;
        }
        return Ok(Toggle::Start(lock));
    }
    File::create(&off_file)?;
    Ok(if request_stop(state_dir, timeout)? { Toggle::Stopped } else { Toggle::StillRunning })
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
    ViewLost,
}

enum Tick {
    Synced,
    ViewLost,
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
                self.sync.clear_all(&self.herdr);
                let _ = self.herdr.call("agent.view.clear", json!({ "source": self.source }));
                return Exit::Stopped;
            }
            match self.tick() {
                Ok(Tick::Synced) => socket_failures = 0,
                Ok(Tick::ViewLost) => {
                    self.sync.clear_all(&self.herdr);
                    return Exit::ViewLost;
                }
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

    fn view_is_ours(&self) -> Result<bool, Error> {
        let probe_source = format!("{}.probe", self.source);
        let view = self.herdr.call("agent.view.clear", json!({ "source": probe_source }))?;
        Ok(view["active"] == true && view["source"] == self.source.as_str())
    }

    fn tick(&mut self) -> Result<Tick, Error> {
        if !self.view_is_ours()? {
            return Ok(Tick::ViewLost);
        }
        let mut result = self.herdr.call("session.snapshot", json!({}))?;
        let snapshot = result.get_mut("snapshot").map(Value::take).unwrap_or(Value::Null);
        let agents = model::agents(snapshot).map_err(|error| Error::Protocol(error.to_string()))?;
        let rows = layout(&agents, |cwd| self.projects.name_for(cwd));
        for error in self.sync.apply(&self.herdr, rows) {
            eprintln!("groups: {error}");
        }
        Ok(Tick::Synced)
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
        calls: Rc<RefCell<Vec<(String, Value)>>>,
        view_source: Rc<RefCell<Option<String>>>,
        socket_gone: bool,
        reject_view: bool,
    }

    impl FakeHerdr {
        fn count(&self, method: &str) -> usize {
            self.calls.borrow().iter().filter(|(called, _)| called == method).count()
        }

        fn last_method(&self) -> String {
            self.calls.borrow().last().unwrap().0.clone()
        }

        fn hand_view_to(&self, source: Option<&str>) {
            *self.view_source.borrow_mut() = source.map(String::from);
        }

        fn view_response(&self) -> Value {
            match self.view_source.borrow().as_deref() {
                Some(source) => json!({ "type": "agent_view", "active": true, "source": source }),
                None => json!({ "type": "agent_view", "active": false }),
            }
        }
    }

    impl Herdr for FakeHerdr {
        fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
            self.calls.borrow_mut().push((method.to_string(), params.clone()));
            match method {
                "agent.view.set" if self.reject_view => {
                    Err(Error::Herdr { code: "plugin_disabled".into(), message: String::new() })
                }
                "agent.view.set" => {
                    self.hand_view_to(params["source"].as_str());
                    Ok(self.view_response())
                }
                "agent.view.clear" => {
                    let owner = self.view_source.borrow().clone();
                    if params["source"].is_null() || params["source"].as_str() == owner.as_deref() {
                        self.hand_view_to(None);
                    }
                    Ok(self.view_response())
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
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut relocked = lock(dir.path()).unwrap();
        while relocked.is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
            relocked = lock(dir.path()).unwrap();
        }
        assert!(relocked.is_some());
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
        assert_eq!(herdr.last_method(), "agent.view.clear");
        assert_eq!(*herdr.view_source.borrow(), None);
    }

    fn lose_view_after(
        herdr: &FakeHerdr,
        state_dir: &Path,
        ticks: usize,
        new_owner: Option<&'static str>,
    ) -> impl FnMut() {
        let herdr = herdr.clone();
        let mut stop_eventually = stop_after(state_dir, ticks + 3);
        let mut waited = 0;
        move || {
            waited += 1;
            if waited == ticks {
                herdr.hand_view_to(new_owner);
            }
            stop_eventually();
        }
    }

    fn cleared_panes(herdr: &FakeHerdr) -> usize {
        herdr
            .calls
            .borrow()
            .iter()
            .filter(|(method, params)| method == "pane.report_metadata" && params["tokens"]["header"].is_null())
            .count()
    }

    #[test]
    fn lost_view_clears_tokens_and_exits() {
        let dir = TempDir::new().unwrap();
        let herdr = FakeHerdr::default();
        let exit =
            Daemon::new(herdr.clone(), "plugin:test".into(), dir.path()).run(lose_view_after(&herdr, dir.path(), 1, None));
        assert_eq!(exit, Exit::ViewLost);
        assert_eq!(herdr.count("session.snapshot"), 1);
        assert_eq!(cleared_panes(&herdr), 1);
    }

    #[test]
    fn view_taken_by_another_source_counts_as_lost() {
        let dir = TempDir::new().unwrap();
        let herdr = FakeHerdr::default();
        let exit = Daemon::new(herdr.clone(), "plugin:test".into(), dir.path())
            .run(lose_view_after(&herdr, dir.path(), 1, Some("plugin:other")));
        assert_eq!(exit, Exit::ViewLost);
        assert_eq!(*herdr.view_source.borrow(), Some("plugin:other".into()));
    }

    #[test]
    fn checking_the_view_leaves_it_in_place() {
        let dir = TempDir::new().unwrap();
        let herdr = FakeHerdr::default();
        Daemon::new(herdr.clone(), "plugin:test".into(), dir.path()).run(lose_view_after(&herdr, dir.path(), 2, None));
        assert_eq!(herdr.count("session.snapshot"), 2);
    }

    #[test]
    fn stop_request_outlives_the_stopped_instance() {
        let dir = TempDir::new().unwrap();
        Daemon::new(FakeHerdr::default(), "plugin:test".into(), dir.path()).run(stop_after(dir.path(), 1));
        assert!(dir.path().join(STOP_FILE).exists());
    }

    #[test]
    fn start_waits_for_a_stopping_instance() {
        let dir = TempDir::new().unwrap();
        let held = lock(dir.path()).unwrap().unwrap();
        write_stop_request(dir.path()).unwrap();
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            drop(held);
        });
        let started = Instant::now();
        assert!(lock_for_start(dir.path(), Duration::from_secs(3)).unwrap().is_some());
        assert!(started.elapsed() >= Duration::from_millis(200));
        releaser.join().unwrap();
    }

    #[test]
    fn start_gives_up_at_once_without_a_pending_stop() {
        let dir = TempDir::new().unwrap();
        let _held = lock(dir.path()).unwrap().unwrap();
        let started = Instant::now();
        assert!(lock_for_start(dir.path(), Duration::from_secs(3)).unwrap().is_none());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn stop_succeeds_when_a_replacement_already_started() {
        let dir = TempDir::new().unwrap();
        let _replacement = lock(dir.path()).unwrap().unwrap();
        let state_dir = dir.path().to_path_buf();
        let consumer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            fs::remove_file(state_dir.join(STOP_FILE)).unwrap();
        });
        assert!(request_stop(dir.path(), Duration::from_secs(3)).unwrap());
        consumer.join().unwrap();
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
    fn toggle_while_running_turns_groups_off() {
        let dir = TempDir::new().unwrap();
        let held = lock(dir.path()).unwrap().unwrap();
        let stop_file = dir.path().join(STOP_FILE);
        let daemon = thread::spawn(move || {
            while !stop_file.exists() {
                thread::sleep(Duration::from_millis(10));
            }
            drop(held);
        });
        assert!(matches!(toggle(dir.path(), Duration::from_secs(3)).unwrap(), Toggle::Stopped));
        assert!(dir.path().join(OFF_FILE).exists());
        daemon.join().unwrap();
    }

    #[test]
    fn toggle_while_idle_turns_groups_back_on() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join(OFF_FILE)).unwrap();
        assert!(matches!(toggle(dir.path(), Duration::from_secs(3)).unwrap(), Toggle::Start(_)));
        assert!(!dir.path().join(OFF_FILE).exists());
    }

    #[test]
    fn toggle_on_keeps_the_lock_for_the_new_daemon() {
        let dir = TempDir::new().unwrap();
        let _toggled = toggle(dir.path(), Duration::from_secs(3)).unwrap();
        assert!(lock(dir.path()).unwrap().is_none());
    }

    #[test]
    fn start_does_nothing_while_groups_is_off() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join(OFF_FILE)).unwrap();
        assert!(lock_for_start(dir.path(), Duration::from_secs(3)).unwrap().is_none());
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
