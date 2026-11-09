//! Static project declarations for discovery. Discovery and watching themselves live in the plugin
//! SDK (`konoma_plugin::host::project`), shared with the standalone app; this module only decides
//! which declarations konoma offers it, from its own registry.

#[cfg(test)]
#[path = "vcs_detection_tests.rs"]
mod vcs_detection_tests;

use std::path::Path;
use std::sync::Arc;

#[cfg(test)]
use konoma_plugin::host::project::ProjectDeclaration;
use konoma_plugin::host::project::ProjectDeclarationSet;
pub(crate) use konoma_plugin::host::project::{MatchedProject, ProjectDetection};
use konoma_plugin::Plugin;

/// Start discovery for `root` unless it already runs for it. The declarations are built only when
/// the root changed: a change of `disabled` alone does not restart detection.
pub(crate) fn ensure(detection: &mut ProjectDetection, root: &Path, disabled: &[String]) {
    if detection.is_for(root) {
        return;
    }
    detection.ensure(root, declarations(disabled));
}

/// Process plugins may advertise projects, but discovery is restricted to static plugins.
fn declarations(disabled: &[String]) -> ProjectDeclarationSet {
    let external: Vec<Arc<dyn Plugin>> = disabled
        .iter()
        .filter_map(|id| super::external::token_of(id))
        .flat_map(super::external::plugins_of)
        .map(|p| p as Arc<dyn Plugin>)
        .collect();
    super::with_registry(disabled, |r| static_declarations(r, &external))
}

fn static_declarations(
    registry: &konoma_plugin::registry::Registry,
    external: &[Arc<dyn Plugin>],
) -> ProjectDeclarationSet {
    konoma_plugin::host::project::detection::project_declarations(registry, |plugin| {
        external.iter().all(|p| !Arc::ptr_eq(p, plugin))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::test_plugins::{self, ProjectFixture};
    use crate::plugins::test_projects::{ProjectLog, Screens};
    use konoma_plugin::project::{DetectionMarker, MarkerKind, ProjectDecl, ProjectScreen};
    use std::sync::Mutex;

    fn declaration(id: &str, marker: &str, kind: MarkerKind) -> ProjectDeclaration {
        ProjectDeclaration {
            plugin: id.into(),
            project: ProjectDecl::new()
                .marker(DetectionMarker::new(marker, kind))
                .screen(
                    ProjectScreen::new("items", "Items")
                        .key('i')
                        .row_kind("item"),
                ),
        }
    }

    #[test]
    fn project_detection_uses_static_registration_and_respects_disabled_plugins() {
        let decl = declaration("static-project", "project.marker", MarkerKind::Any);
        let marker = test_plugins::register(
            "project_detection_static",
            Arc::new(ProjectFixture {
                id: "static-project",
                project: decl.project.clone(),
                calls: Arc::default(),
                navigation: None,
            }),
        );
        let enabled = declarations(std::slice::from_ref(&marker));
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].plugin, "static-project");
        assert!(declarations(&[marker, "static-project".into()]).is_empty());
    }

    #[test]
    fn project_static_discovery_excludes_the_process_instance_not_a_colliding_id() {
        let plugin = || -> Arc<dyn Plugin> {
            Arc::new(ProjectFixture {
                id: "project-identity",
                project: declaration("project-identity", "project.marker", MarkerKind::Any).project,
                calls: Arc::default(),
                navigation: None,
            })
        };
        let linked = plugin();
        let refused = plugin();
        let mut registry = konoma_plugin::registry::Registry::new();
        registry
            .register(linked.clone(), konoma_plugin::registry::PluginOrigin::User)
            .unwrap();
        assert_eq!(static_declarations(&registry, &[refused]).len(), 1);
        assert!(static_declarations(&registry, &[linked]).is_empty());
    }

    #[test]
    fn project_ensure_builds_declarations_only_for_a_new_root() {
        let dir = crate::test_support::unique_tmp("project_ensure_host");
        std::fs::create_dir_all(&dir).unwrap();
        let marker = test_plugins::register(
            "project_ensure_host",
            Arc::new(ProjectFixture {
                id: "ensure-host",
                project: declaration("ensure-host", "project.marker", MarkerKind::Any).project,
                calls: Arc::default(),
                navigation: None,
            }),
        );
        let mut detection = ProjectDetection::default();
        ensure(&mut detection, &dir, std::slice::from_ref(&marker));
        assert!(detection.pending());
        let before = detection.clone();
        // Disabling the plugin does not restart detection while the root stays.
        ensure(&mut detection, &dir, &[marker, "ensure-host".into()]);
        assert!(detection.shares_worker(&before));
        assert!(detection.pending());
    }

    #[test]
    fn project_ensure_same_root_does_not_rebuild_declarations() {
        let dir = crate::test_support::unique_tmp("project_ensure_host_manifest");
        std::fs::create_dir_all(&dir).unwrap();
        let mut detection = ProjectDetection::default();
        ensure(&mut detection, &dir, &[]);

        let log = Arc::new(Mutex::new(ProjectLog::default()));
        let marker = test_plugins::register(
            "project_ensure_host_manifest",
            Arc::new(Screens {
                log: log.clone(),
                read_only: false,
                hold: None,
                started: None,
                outcome: None,
            }),
        );
        // This fresh marker makes a registry cache miss observable if the host guard is lost.
        ensure(&mut detection, &dir, std::slice::from_ref(&marker));
        assert_eq!(log.lock().unwrap().manifests, 0);

        // Prove the spy is registered and counts manifest calls when declarations are rebuilt.
        let rebuilt = declarations(&[marker]);
        assert!(rebuilt.iter().any(|decl| decl.plugin == "project-screens"));
        assert!(log.lock().unwrap().manifests > 0);
    }
}
