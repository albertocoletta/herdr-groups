use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub pane_id: String,
    pub cwd: PathBuf,
    pub title: String,
    pub position: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub workspace: usize,
    pub tab: usize,
    pub pane: usize,
}

#[derive(Deserialize)]
struct Snapshot {
    agents: Vec<RawAgent>,
    workspaces: Vec<WorkspaceRef>,
    tabs: Vec<TabRef>,
    panes: Vec<PaneRef>,
}

#[derive(Deserialize)]
struct RawAgent {
    agent: Option<String>,
    pane_id: String,
    tab_id: String,
    workspace_id: String,
    cwd: Option<PathBuf>,
    foreground_cwd: Option<PathBuf>,
    terminal_title_stripped: Option<String>,
}

#[derive(Deserialize)]
struct WorkspaceRef {
    workspace_id: String,
}

#[derive(Deserialize)]
struct TabRef {
    tab_id: String,
}

#[derive(Deserialize)]
struct PaneRef {
    pane_id: String,
}

pub fn agents(snapshot: serde_json::Value) -> Result<Vec<Agent>, serde_json::Error> {
    let snapshot: Snapshot = serde_json::from_value(snapshot)?;
    let workspaces = index_of(snapshot.workspaces.into_iter().map(|w| w.workspace_id));
    let tabs = index_of(snapshot.tabs.into_iter().map(|t| t.tab_id));
    let panes = index_of(snapshot.panes.into_iter().map(|p| p.pane_id));
    let position_in = |index: &HashMap<String, usize>, id: &str| index.get(id).copied().unwrap_or(usize::MAX);

    Ok(snapshot
        .agents
        .into_iter()
        .filter_map(|raw| {
            let cwd = raw.foreground_cwd.or(raw.cwd)?;
            let title = raw
                .terminal_title_stripped
                .filter(|title| !title.trim().is_empty())
                .or(raw.agent)
                .unwrap_or_default();
            Some(Agent {
                position: Position {
                    workspace: position_in(&workspaces, &raw.workspace_id),
                    tab: position_in(&tabs, &raw.tab_id),
                    pane: position_in(&panes, &raw.pane_id),
                },
                pane_id: raw.pane_id,
                cwd,
                title,
            })
        })
        .collect())
}

fn index_of(ids: impl Iterator<Item = String>) -> HashMap<String, usize> {
    ids.enumerate().map(|(index, id)| (id, index)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Agent> {
        let json = include_str!("../tests/fixtures/snapshot.json");
        agents(serde_json::from_str(json).unwrap()).unwrap()
    }

    fn agent<'a>(agents: &'a [Agent], pane_id: &str) -> &'a Agent {
        agents.iter().find(|agent| agent.pane_id == pane_id).unwrap()
    }

    #[test]
    fn parses_every_agent() {
        assert_eq!(fixture().len(), 3);
    }

    #[test]
    fn prefers_foreground_cwd() {
        assert_eq!(agent(&fixture(), "wA:p2").cwd, PathBuf::from("/home/dev/projects/api/src"));
    }

    #[test]
    fn falls_back_to_pane_cwd() {
        assert_eq!(agent(&fixture(), "wB:p1").cwd, PathBuf::from("/home/dev/projects/web"));
    }

    #[test]
    fn uses_stripped_terminal_title() {
        assert_eq!(agent(&fixture(), "wA:p2").title, "Refactor auth");
    }

    #[test]
    fn falls_back_to_agent_name_without_title() {
        assert_eq!(agent(&fixture(), "wB:p1").title, "codex");
    }

    #[test]
    fn positions_follow_display_order() {
        let agents = fixture();
        assert_eq!(agent(&agents, "wB:p1").position, Position { workspace: 0, tab: 0, pane: 0 });
        assert_eq!(agent(&agents, "wA:p1").position, Position { workspace: 1, tab: 1, pane: 1 });
        assert_eq!(agent(&agents, "wA:p2").position, Position { workspace: 1, tab: 1, pane: 2 });
    }

    #[test]
    fn unknown_ids_sort_last() {
        let snapshot = serde_json::json!({
            "agents": [{ "pane_id": "wZ:p1", "tab_id": "wZ:t1", "workspace_id": "wZ", "cwd": "/tmp" }],
            "workspaces": [], "tabs": [], "panes": []
        });
        let position = agents(snapshot).unwrap()[0].position;
        assert_eq!(position, Position { workspace: usize::MAX, tab: usize::MAX, pane: usize::MAX });
    }
}
