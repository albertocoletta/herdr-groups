use std::collections::BTreeMap;
use std::path::Path;

use crate::model::Agent;
use crate::project::Project;

pub const SPACER: &str = "\u{200b}";
pub const TOKENS: [&str; 4] = ["order", "header", "worktree", "gap"];

pub type Tokens = BTreeMap<&'static str, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub pane_id: String,
    pub tokens: Tokens,
}

struct Member<'a> {
    agent: &'a Agent,
    worktree: Option<String>,
}

struct Group<'a> {
    name: String,
    members: Vec<Member<'a>>,
}

pub fn layout(agents: &[Agent], mut project_of: impl FnMut(&Path) -> Project) -> Vec<Row> {
    let mut sorted: Vec<&Agent> = agents.iter().collect();
    sorted.sort_by_key(|agent| agent.position);

    let mut groups: Vec<Group> = Vec::new();
    for agent in sorted {
        let Project { name, worktree } = project_of(&agent.cwd);
        let member = Member { agent, worktree };
        match groups.iter_mut().find(|group| group.name == name) {
            Some(group) => group.members.push(member),
            None => groups.push(Group { name, members: vec![member] }),
        }
    }

    groups
        .iter()
        .enumerate()
        .flat_map(|(group_index, group)| {
            group.members.iter().enumerate().map(move |(member_index, member)| Row {
                pane_id: member.agent.pane_id.clone(),
                tokens: tokens(group_index, member_index, &group.name, member),
            })
        })
        .collect()
}

fn tokens(group_index: usize, member_index: usize, group_name: &str, member: &Member) -> Tokens {
    let opens_group = member_index == 0;
    let mut tokens = Tokens::new();
    tokens.insert("order", format!("{group_index:04}-{member_index:04}"));
    tokens.insert("header", if opens_group { group_name.to_string() } else { SPACER.to_string() });
    if let Some(folder) = &member.worktree {
        tokens.insert("worktree", format!("wt {folder}"));
    }
    if opens_group && group_index > 0 {
        tokens.insert("gap", SPACER.to_string());
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Position;
    use std::path::PathBuf;

    fn agent(pane_id: &str, cwd: &str, workspace: usize, pane: usize) -> Agent {
        Agent {
            pane_id: pane_id.to_string(),
            cwd: PathBuf::from(cwd),
            position: Position { workspace, tab: workspace, pane },
        }
    }

    fn folder(path: &Path) -> Project {
        Project { name: path.file_name().unwrap().to_string_lossy().into_owned(), worktree: None }
    }

    fn with_worktrees(path: &Path) -> Project {
        match path.to_str().unwrap().split_once("@") {
            Some((repo, worktree)) => Project { name: folder(Path::new(repo)).name, worktree: Some(worktree.into()) },
            None => folder(path),
        }
    }

    fn token<'a>(rows: &'a [Row], pane_id: &str, name: &str) -> Option<&'a str> {
        let row = rows.iter().find(|row| row.pane_id == pane_id).unwrap();
        row.tokens.get(name).map(String::as_str)
    }

    fn panes_in_order(rows: &[Row]) -> Vec<&str> {
        let mut orders: Vec<(&str, &str)> = rows.iter().map(|r| (r.tokens["order"].as_str(), r.pane_id.as_str())).collect();
        orders.sort();
        orders.into_iter().map(|(_, pane)| pane).collect()
    }

    fn sample() -> Vec<Row> {
        layout(
            &[
                agent("api-2", "/p/api", 1, 3),
                agent("web-1", "/p/web", 0, 0),
                agent("api-1", "/p/api", 1, 2),
                agent("api-3", "/p/api", 2, 4),
            ],
            folder,
        )
    }

    #[test]
    fn first_member_of_each_group_gets_the_group_name() {
        let rows = sample();
        assert_eq!(token(&rows, "web-1", "header"), Some("web"));
        assert_eq!(token(&rows, "api-1", "header"), Some("api"));
    }

    #[test]
    fn other_members_get_a_spacer_header() {
        let rows = sample();
        assert_eq!(token(&rows, "api-2", "header"), Some(SPACER));
        assert_eq!(token(&rows, "api-3", "header"), Some(SPACER));
    }

    #[test]
    fn spacer_survives_whitespace_trimming() {
        assert_eq!(SPACER.trim(), SPACER);
        assert!(!SPACER.is_empty());
    }

    #[test]
    fn groups_follow_workspace_order() {
        assert_eq!(panes_in_order(&sample()), ["web-1", "api-1", "api-2", "api-3"]);
    }

    #[test]
    fn group_spanning_workspaces_stays_together() {
        let rows = layout(
            &[
                agent("api-1", "/p/api", 0, 0),
                agent("web-1", "/p/web", 1, 1),
                agent("api-2", "/p/api", 2, 2),
            ],
            folder,
        );
        assert_eq!(panes_in_order(&rows), ["api-1", "api-2", "web-1"]);
    }

    #[test]
    fn worktree_joins_its_repo_group() {
        let rows = layout(
            &[
                agent("api-1", "/p/api", 0, 0),
                agent("web-1", "/p/web", 1, 1),
                agent("api-fix", "/p/api@fix-x", 2, 2),
            ],
            with_worktrees,
        );
        assert_eq!(panes_in_order(&rows), ["api-1", "api-fix", "web-1"]);
        assert_eq!(token(&rows, "api-fix", "header"), Some(SPACER));
    }

    #[test]
    fn only_worktree_rows_get_a_worktree_label() {
        let rows = layout(&[agent("api-1", "/p/api", 0, 0), agent("api-fix", "/p/api@fix-x", 1, 1)], with_worktrees);
        assert_eq!(token(&rows, "api-1", "worktree"), None);
        assert_eq!(token(&rows, "api-fix", "worktree"), Some("wt fix-x"));
    }

    #[test]
    fn gap_opens_every_group_but_the_first() {
        let rows = sample();
        assert_eq!(token(&rows, "web-1", "gap"), None);
        assert_eq!(token(&rows, "api-1", "gap"), Some(SPACER));
        assert_eq!(token(&rows, "api-2", "gap"), None);
        assert_eq!(token(&rows, "api-3", "gap"), None);
    }

    #[test]
    fn every_token_is_listed_in_tokens() {
        let rows = layout(&[agent("web-1", "/p/web", 0, 0), agent("api-fix", "/p/api@fix-x", 1, 1)], with_worktrees);
        for row in rows {
            assert!(row.tokens.keys().all(|name| TOKENS.contains(name)), "{:?}", row.tokens);
        }
    }

    #[test]
    fn layout_is_stable_across_runs() {
        assert_eq!(sample(), sample());
    }
}
