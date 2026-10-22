use std::path::Path;

use crate::model::Agent;

const ZERO_WIDTH_SPACE: char = '\u{200b}';
const HEADER_ROW_INDENT: &str = "  ";
const FIRST_ROW_INDENT: &str = "    ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub pane_id: String,
    pub order: String,
    pub header: Option<String>,
    pub label: String,
}

struct Group<'a> {
    name: String,
    members: Vec<&'a Agent>,
}

pub fn layout(agents: &[Agent], mut name_of: impl FnMut(&Path) -> String) -> Vec<Row> {
    let mut sorted: Vec<&Agent> = agents.iter().collect();
    sorted.sort_by_key(|agent| agent.position);

    let mut groups: Vec<Group> = Vec::new();
    for agent in sorted {
        let name = name_of(&agent.cwd);
        match groups.iter_mut().find(|group| group.name == name) {
            Some(group) => group.members.push(agent),
            None => groups.push(Group { name, members: vec![agent] }),
        }
    }

    groups
        .iter()
        .enumerate()
        .flat_map(|(group_index, group)| {
            let last = group.members.len() - 1;
            group.members.iter().enumerate().map(move |(member_index, agent)| {
                let first = member_index == 0;
                Row {
                    pane_id: agent.pane_id.clone(),
                    order: format!("{group_index:04}-{member_index:04}"),
                    header: first.then(|| group.name.clone()),
                    label: label(&agent.title, first, member_index == last),
                }
            })
        })
        .collect()
}

fn label(title: &str, has_header: bool, is_last: bool) -> String {
    let indent = if has_header { HEADER_ROW_INDENT } else { FIRST_ROW_INDENT };
    let branch = if is_last { '└' } else { '├' };
    format!("{ZERO_WIDTH_SPACE}{indent}{branch} {title}")
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
            title: format!("title {pane_id}"),
            position: Position { workspace, tab: workspace, pane },
        }
    }

    fn folder(path: &Path) -> String {
        path.file_name().unwrap().to_string_lossy().into_owned()
    }

    fn row<'a>(rows: &'a [Row], pane_id: &str) -> &'a Row {
        rows.iter().find(|row| row.pane_id == pane_id).unwrap()
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
    fn header_only_on_first_member_of_each_group() {
        let rows = sample();
        assert_eq!(row(&rows, "web-1").header.as_deref(), Some("web"));
        assert_eq!(row(&rows, "api-1").header.as_deref(), Some("api"));
        assert_eq!(row(&rows, "api-2").header, None);
        assert_eq!(row(&rows, "api-3").header, None);
    }

    #[test]
    fn last_member_gets_the_closing_branch() {
        let rows = sample();
        assert!(row(&rows, "api-1").label.contains('├'));
        assert!(row(&rows, "api-2").label.contains('├'));
        assert!(row(&rows, "api-3").label.contains('└'));
        assert!(row(&rows, "web-1").label.contains('└'));
    }

    #[test]
    fn rows_without_header_get_two_more_spaces() {
        let rows = sample();
        assert_eq!(row(&rows, "api-1").label, "\u{200b}  ├ title api-1");
        assert_eq!(row(&rows, "api-2").label, "\u{200b}    ├ title api-2");
    }

    #[test]
    fn groups_follow_workspace_order() {
        let rows = sample();
        let mut orders: Vec<(&str, &str)> = rows.iter().map(|r| (r.order.as_str(), r.pane_id.as_str())).collect();
        orders.sort();
        let panes: Vec<&str> = orders.into_iter().map(|(_, pane)| pane).collect();
        assert_eq!(panes, ["web-1", "api-1", "api-2", "api-3"]);
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
        let mut orders: Vec<(&str, &str)> = rows.iter().map(|r| (r.order.as_str(), r.pane_id.as_str())).collect();
        orders.sort();
        let panes: Vec<&str> = orders.into_iter().map(|(_, pane)| pane).collect();
        assert_eq!(panes, ["api-1", "api-2", "web-1"]);
    }

    #[test]
    fn layout_is_stable_across_runs() {
        assert_eq!(sample(), sample());
    }
}
