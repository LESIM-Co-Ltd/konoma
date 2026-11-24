use konoma_plugin::ProjectView;
use std::path::{Path, PathBuf};

pub(super) fn display(
    path: &Path,
    project_root: &Path,
    context: Option<&ProjectView>,
    style: &str,
) -> String {
    let home = || {
        context
            .and_then(|c| c.home_dir.as_deref())
            .and_then(|home| path.strip_prefix(home).ok())
            .map(|relative| {
                if relative.as_os_str().is_empty() {
                    "~".into()
                } else {
                    format!("~/{}", relative.display())
                }
            })
            .unwrap_or_else(|| path.display().to_string())
    };
    match style {
        "full" => path.display().to_string(),
        "home" => home(),
        _ => {
            let Some(open_dir) = context.and_then(|c| c.open_dir.as_deref()) else {
                return path
                    .strip_prefix(project_root)
                    .unwrap_or(path)
                    .display()
                    .to_string();
            };
            if path.starts_with(open_dir) {
                path.strip_prefix(open_dir.parent().unwrap_or(open_dir))
                    .ok()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(home)
            } else {
                let base: Vec<_> = open_dir.components().collect();
                let target: Vec<_> = path.components().collect();
                let shared = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
                if shared == 0 && (open_dir.is_absolute() || path.is_absolute()) {
                    return home();
                }
                let mut relative = PathBuf::new();
                for _ in shared..base.len() {
                    relative.push("..");
                }
                for component in &target[shared..] {
                    relative.push(component.as_os_str());
                }
                if relative.as_os_str().is_empty() {
                    home()
                } else {
                    relative.display().to_string()
                }
            }
        }
    }
}
