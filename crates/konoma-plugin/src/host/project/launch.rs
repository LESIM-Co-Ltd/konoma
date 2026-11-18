//! Pure launch values; hosts retain backend discovery and process execution.
use std::path::{Path, PathBuf};

use crate::launch::{Attempt, Opener};

/// The backend tool and its repository working directory, ready for terminal hand-off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectToolLaunch {
    pub attempt: Attempt,
    pub cwd: PathBuf,
}

/// Split the resolved backend command from `ProjectSettings::tool_command`.
/// The host discovers the workdir and checks its disabled gate before building the plan.
pub fn project_tool_launch(command: &str, workdir: &Path) -> Option<ProjectToolLaunch> {
    let mut parts = command.split_whitespace().map(str::to_owned);
    Some(ProjectToolLaunch {
        attempt: Attempt {
            opener: Opener::SystemDefault,
            prog: parts.next()?,
            args: parts.collect(),
        },
        cwd: workdir.to_path_buf(),
    })
}
