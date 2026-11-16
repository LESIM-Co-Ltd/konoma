//! A file opened as a preview must come out the same whether its kind is decided on the spot (an
//! `App` without the preview resolution worker, as the tests build it) or on the worker (as `main`
//! attaches it). The two roads share `App::apply_resolved_preview`; these tests pin that every
//! file a standard plugin takes is opened and drawn the same on both.

use super::*;

/// The entries of `samples/`: the files this test opens, and the directories it skips
/// (`code/` holds files other tests open one by one; a directory is not a preview target).
fn sample_files() -> (Vec<std::path::PathBuf>, Vec<std::path::PathBuf>) {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples");
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for e in std::fs::read_dir(&dir).expect("samples/") {
        let p = e.unwrap().path();
        if p.is_file() {
            files.push(p);
        } else {
            skipped.push(p);
        }
    }
    files.sort();
    (files, skipped)
}

/// The drawn rows of `s`, a wide symbol counted once (the cell behind it keeps whatever an
/// earlier frame left there, which is not what is drawn).
fn rows(s: &Sim) -> Vec<String> {
    super::table_parity::buffer_rows(s.term.backend().buffer())
}

/// Opens `file` through the tree and waits until every job it started has landed.
#[track_caller]
fn open_settled(root: &std::path::Path, file: &str, worker: bool) -> Sim {
    let base = Sim::with_config_sized(root, cfg_en(), 110, 40);
    let mut s = if worker {
        base.with_media_and_preview_worker()
    } else {
        base.with_media()
    };
    s.select(file);
    s.enter();
    s.settle_inline_images(file, std::time::Duration::from_secs(120));
    s.draw();
    s
}

#[test]
fn every_sample_is_drawn_the_same_with_and_without_the_preview_worker() {
    let (files, skipped) = sample_files();
    eprintln!("skipped (directories, not previews of themselves): {skipped:?}");
    assert!(
        files.len() >= 30,
        "the samples directory was not read: {} files",
        files.len()
    );
    // A sample with a remote picture starts a download: its cache is a sandbox, never the real one.
    let cache = sandbox("pwp_cache");
    crate::test_support::set_test_cache_root(cache.to_path_buf());
    let mut failures = Vec::new();
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let dir = sandbox("pwp");
        std::fs::copy(f, dir.join(&name)).unwrap();
        let root = canon(&dir);
        let sync = open_settled(&root, &name, false);
        let worker = open_settled(&root, &name, true);
        let (a, b) = (rows(&sync), rows(&worker));
        if a != b {
            let i = a
                .iter()
                .zip(&b)
                .position(|(x, y)| x != y)
                .unwrap_or(a.len().min(b.len()));
            failures.push(format!(
                "{name}: drawn differently with the worker; first differing row {i}:\n  sync:   {:?}\n  worker: {:?}",
                a.get(i),
                b.get(i)
            ));
        }
        // A table is drawn as a table (the title carries the row position), not as raw text.
        if name.ends_with(".csv") || name.ends_with(".tsv") {
            for (road, s) in [("sync", &sync), ("worker", &worker)] {
                if !s.screen().contains("r1/") {
                    failures.push(format!("{name}: the {road} road does not draw the table"));
                }
            }
        }
        // A file a standard plugin takes is never the failure screen on either road.
        for (road, s) in [("sync", &sync), ("worker", &worker)] {
            if s.screen().contains("can not preview") {
                failures.push(format!("{name}: the {road} road draws `can not preview`"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn with_the_worker_a_tree_enter_and_ctrl_n_open_markdown_then_a_table() {
    let dir = sandbox("pwp_keys");
    std::fs::write(dir.join("a.md"), "# Heading One\n\nbody text\n").unwrap();
    std::fs::write(dir.join("b.csv"), "col1,col2\n1,2\n3,4\n").unwrap();
    let root = canon(&dir);
    let mut s = Sim::with_config_sized(&root, cfg_en(), 110, 30).with_media_and_preview_worker();
    s.select("a.md");
    s.enter();
    s.settle_inline_images("a.md", std::time::Duration::from_secs(120));
    s.see("Heading One");
    s.dont_see("can not preview");
    s.ctrl('n');
    s.settle_inline_images("b.csv", std::time::Duration::from_secs(120));
    s.see("r1/");
    s.see("col1");
    s.dont_see("can not preview");
}

#[test]
fn with_the_worker_a_tree_enter_on_a_csv_draws_the_table() {
    let dir = sandbox("pwp_csv");
    std::fs::write(dir.join("b.csv"), "col1,col2\n1,2\n3,4\n").unwrap();
    let root = canon(&dir);
    let mut s = Sim::with_config_sized(&root, cfg_en(), 110, 30).with_media_and_preview_worker();
    s.select("b.csv");
    s.enter();
    s.settle_inline_images("b.csv", std::time::Duration::from_secs(120));
    s.see("r1/");
    s.see("col1");
}
