use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::herdr::{Error, Herdr};
use crate::layout::{Row, TOKENS};

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
    TOKENS
        .iter()
        .filter_map(|&token| {
            let old = previous.and_then(|p| p.tokens.get(token));
            let new = row.tokens.get(token);
            (old != new).then(|| (token.to_string(), new.map_or(Value::Null, |new| Value::String(new.clone()))))
        })
        .collect()
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

    fn row(pane_id: &str, order: &str, header: &str) -> Row {
        let tokens = [("order", order.to_string()), ("header", header.to_string())].into_iter().collect();
        Row { pane_id: pane_id.into(), tokens }
    }

    fn worktree_row(pane_id: &str, worktree: &str) -> Row {
        let mut row = row(pane_id, "0000-0000", "api");
        row.tokens.insert("worktree", worktree.into());
        row
    }

    #[test]
    fn first_report_sends_every_token() {
        let herdr = FakeHerdr::default();
        Sync::new("plugin:test").apply(&herdr, vec![row("w1:p1", "0000-0000", "api")]);
        let calls = herdr.take();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["source"], "plugin:test");
        assert_eq!(calls[0]["tokens"], json!({ "order": "0000-0000", "header": "api" }));
    }

    #[test]
    fn unchanged_rows_send_nothing() {
        let herdr = FakeHerdr::default();
        let mut sync = Sync::new("plugin:test");
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", "api")]);
        herdr.take();
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", "api")]);
        assert!(herdr.take().is_empty());
    }

    #[test]
    fn only_changed_tokens_are_sent() {
        let herdr = FakeHerdr::default();
        let mut sync = Sync::new("plugin:test");
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", "api")]);
        herdr.take();
        sync.apply(&herdr, vec![row("w1:p1", "0000-0001", "api")]);
        assert_eq!(herdr.take()[0]["tokens"], json!({ "order": "0000-0001" }));
    }

    #[test]
    fn header_change_is_sent() {
        let herdr = FakeHerdr::default();
        let mut sync = Sync::new("plugin:test");
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", "api")]);
        herdr.take();
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", "\u{200b}")]);
        assert_eq!(herdr.take()[0]["tokens"], json!({ "header": "\u{200b}" }));
    }

    #[test]
    fn vanished_panes_are_cleared() {
        let herdr = FakeHerdr::default();
        let mut sync = Sync::new("plugin:test");
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", "api")]);
        herdr.take();
        sync.apply(&herdr, vec![]);
        let calls = herdr.take();
        assert_eq!(calls[0]["pane_id"], "w1:p1");
        assert_eq!(calls[0]["tokens"], json!({ "order": null, "header": null, "worktree": null, "gap": null }));
    }

    #[test]
    fn worktree_is_sent_when_set() {
        let herdr = FakeHerdr::default();
        Sync::new("plugin:test").apply(&herdr, vec![worktree_row("w1:p1", "wt fix-x")]);
        assert_eq!(herdr.take()[0]["tokens"]["worktree"], "wt fix-x");
    }

    #[test]
    fn worktree_is_cleared_when_unset() {
        let herdr = FakeHerdr::default();
        let mut sync = Sync::new("plugin:test");
        sync.apply(&herdr, vec![worktree_row("w1:p1", "wt fix-x")]);
        herdr.take();
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", "api")]);
        let tokens: Vec<Value> = herdr.take().iter().map(|call| call["tokens"].clone()).collect();
        assert_eq!(tokens, [json!({ "worktree": null })]);
    }

    #[test]
    fn failed_report_is_retried_next_time() {
        let failing = FakeHerdr { failing_pane: Some("w1:p1"), ..FakeHerdr::default() };
        let mut sync = Sync::new("plugin:test");
        let errors = sync.apply(&failing, vec![row("w1:p1", "0000-0000", "api")]);
        assert_eq!(errors.len(), 1);

        let herdr = FakeHerdr::default();
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", "api")]);
        assert_eq!(herdr.take().len(), 1);
    }

    #[test]
    fn clear_all_clears_every_reported_pane() {
        let herdr = FakeHerdr::default();
        let mut sync = Sync::new("plugin:test");
        sync.apply(&herdr, vec![row("w1:p1", "0000-0000", "api"), row("w1:p2", "0000-0001", "\u{200b}")]);
        herdr.take();
        sync.clear_all(&herdr);
        let mut panes: Vec<String> = herdr.take().iter().map(|call| call["pane_id"].as_str().unwrap().into()).collect();
        panes.sort();
        assert_eq!(panes, ["w1:p1", "w1:p2"]);
    }
}
