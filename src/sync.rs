use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::herdr::{Error, Herdr};
use crate::layout::Row;

pub const TOKENS: [&str; 3] = ["order", "header", "label"];

pub struct Sync {
    source: String,
    reported: HashMap<String, Row>,
}

impl Sync {
    pub fn new(source: impl Into<String>) -> Self {
        Self { source: source.into(), reported: HashMap::new() }
    }

    pub fn apply(&mut self, herdr: &impl Herdr, rows: Vec<Row>) -> Vec<Error> {
        let mut errors = Vec::new();

        let live: Vec<&str> = rows.iter().map(|row| row.pane_id.as_str()).collect();
        let gone: Vec<String> =
            self.reported.keys().filter(|pane| !live.contains(&pane.as_str())).cloned().collect();
        for pane_id in gone {
            self.reported.remove(&pane_id);
            let cleared = TOKENS.iter().map(|token| (token.to_string(), Value::Null)).collect();
            let _ = self.report(herdr, &pane_id, cleared);
        }

        for row in rows {
            let patch = changes(self.reported.get(&row.pane_id), &row);
            if patch.is_empty() {
                continue;
            }
            match self.report(herdr, &row.pane_id, patch) {
                Ok(()) => {
                    self.reported.insert(row.pane_id.clone(), row);
                }
                Err(error) => errors.push(error),
            }
        }
        errors
    }

    pub fn clear_all(&mut self, herdr: &impl Herdr) {
        for pane_id in std::mem::take(&mut self.reported).into_keys() {
            let cleared = TOKENS.iter().map(|token| (token.to_string(), Value::Null)).collect();
            let _ = self.report(herdr, &pane_id, cleared);
        }
    }

    fn report(&self, herdr: &impl Herdr, pane_id: &str, tokens: Map<String, Value>) -> Result<(), Error> {
        herdr
            .call(
                "pane.report_metadata",
                json!({ "pane_id": pane_id, "source": self.source, "tokens": tokens }),
            )
            .map(|_| ())
    }
}

fn changes(previous: Option<&Row>, row: &Row) -> Map<String, Value> {
    let mut patch = Map::new();
    let mut set = |token: &str, old: Option<&str>, new: Option<&str>| {
        if old != new {
            patch.insert(token.to_string(), new.map_or(Value::Null, |value| Value::String(value.into())));
        }
    };
    set("order", previous.map(|p| p.order.as_str()), Some(&row.order));
    set("header", previous.and_then(|p| p.header.as_deref()), row.header.as_deref());
    set("label", previous.map(|p| p.label.as_str()), Some(&row.label));
    patch
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeHerdr {
        calls: RefCell<Vec<Value>>,
        failing_pane: Option<&'static str>,
    }

    impl Herdr for FakeHerdr {
        fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
            assert_eq!(method, "pane.report_metadata");
            if self.failing_pane.is_some_and(|pane| params["pane_id"] == pane) {
                return Err(Error::Herdr { code: "pane_not_found".into(), message: String::new() });
            }
            self.calls.borrow_mut().push(params);
            Ok(json!({}))
        }
    }

    impl FakeHerdr {
        fn take(&self) -> Vec<Value> {
            std::mem::take(&mut *self.calls.borrow_mut())
        }
    }

    fn row(pane_id: &str, order: &str, header: Option<&str>, label: &str) -> Row {
        Row {
            pane_id: pane_id.into(),
            order: order.into(),
            header: header.map(Into::into),
            label: label.into(),
        }
    }

    #[test]
    fn first_report_sends_every_token() {
        let herdr = FakeHerdr::default();
        Sync::new("plugin:test").apply(&herdr, vec![row("w1:p1", "0000-0000", Some("api"), "a")]);
        let calls = herdr.take();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["source"], "plugin:test");
        assert_eq!(calls[0]["tokens"], json!({ "order": "0000-0000", "header": "api", "label": "a" }));
    }

    #[test]
    fn unchanged_rows_send_nothing() {
        let herdr = FakeHerdr::default();
        let mut sync = Sync::new("plugin:test");
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", Some("api"), "a")]);
        herdr.take();
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", Some("api"), "a")]);
        assert!(herdr.take().is_empty());
    }

    #[test]
    fn only_changed_tokens_are_sent() {
        let herdr = FakeHerdr::default();
        let mut sync = Sync::new("plugin:test");
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", Some("api"), "a")]);
        herdr.take();
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", Some("api"), "b")]);
        assert_eq!(herdr.take()[0]["tokens"], json!({ "label": "b" }));
    }

    #[test]
    fn lost_header_is_cleared() {
        let herdr = FakeHerdr::default();
        let mut sync = Sync::new("plugin:test");
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", Some("api"), "a")]);
        herdr.take();
        sync.apply(&herdr, vec![row("w1:p1", "0000-0001", None, "a")]);
        assert_eq!(herdr.take()[0]["tokens"], json!({ "order": "0000-0001", "header": null }));
    }

    #[test]
    fn vanished_panes_are_cleared() {
        let herdr = FakeHerdr::default();
        let mut sync = Sync::new("plugin:test");
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", Some("api"), "a")]);
        herdr.take();
        sync.apply(&herdr, vec![]);
        let calls = herdr.take();
        assert_eq!(calls[0]["pane_id"], "w1:p1");
        assert_eq!(calls[0]["tokens"], json!({ "order": null, "header": null, "label": null }));
    }

    #[test]
    fn failed_report_is_retried_next_time() {
        let failing = FakeHerdr { failing_pane: Some("w1:p1"), ..FakeHerdr::default() };
        let mut sync = Sync::new("plugin:test");
        let errors = sync.apply(&failing, vec![row("w1:p1", "0000-0000", Some("api"), "a")]);
        assert_eq!(errors.len(), 1);

        let herdr = FakeHerdr::default();
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", Some("api"), "a")]);
        assert_eq!(herdr.take().len(), 1);
    }

    #[test]
    fn clear_all_clears_every_reported_pane() {
        let herdr = FakeHerdr::default();
        let mut sync = Sync::new("plugin:test");
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", Some("api"), "a"), row("w1:p2", "0000-0001", None, "b")]);
        herdr.take();
        sync.clear_all(&herdr);
        let mut panes: Vec<String> = herdr.take().iter().map(|call| call["pane_id"].as_str().unwrap().into()).collect();
        panes.sort();
        assert_eq!(panes, ["w1:p1", "w1:p2"]);
    }
}
