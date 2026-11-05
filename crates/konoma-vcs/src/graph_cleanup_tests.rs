use super::*;

#[test]
fn graph_tab_closed_removes_only_the_closed_tabs_state() {
    let dir = konoma_test_support::unique_tmp("graph_tab_closed");
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = Changes {
        root: dir.to_path_buf(),
        backend: Backend::Git,
        files: vec![],
        graphs: Default::default(),
        detail_meta: Default::default(),
    };
    let req = |tab| ViewReq::new(80, 20).project(ProjectView::new("graph").tab(tab));
    let before = session.view(&req(1), &ViewCx::bare()).unwrap();
    session.view(&req(2), &ViewCx::bare()).unwrap();
    assert_eq!(session.graphs.len(), 2);
    session.tab_closed(2);
    assert!(!session.graphs.contains_key(&2));
    assert!(session.graphs.contains_key(&1));
    let View::List(before) = before else { panic!() };
    let View::List(after) = session.view(&req(1), &ViewCx::bare()).unwrap() else {
        panic!()
    };
    assert_eq!(after, before);
    session.tab_closed(2);
    assert_eq!(session.graphs.len(), 1);
}
