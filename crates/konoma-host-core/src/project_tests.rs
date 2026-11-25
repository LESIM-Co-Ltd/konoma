use super::*;
use DiffView::{Preview, Rendered, Source};

fn representation_rows() -> Vec<(DiffTarget, bool, Option<LandedMedia>, Vec<DiffView>)> {
    vec![
        (
            DiffTarget::Markdown,
            true,
            None,
            vec![Source, Rendered, Preview],
        ),
        (
            DiffTarget::Markdown,
            true,
            Some(LandedMedia::Picture),
            vec![Source, Rendered, Preview],
        ),
        (
            DiffTarget::Markdown,
            true,
            Some(LandedMedia::Svg),
            vec![Source, Rendered, Preview],
        ),
        (
            DiffTarget::Markdown,
            true,
            Some(LandedMedia::NotPicture),
            vec![Source, Rendered, Preview],
        ),
        (DiffTarget::Markdown, false, None, vec![Source, Rendered]),
        (
            DiffTarget::Markdown,
            false,
            Some(LandedMedia::Picture),
            vec![Source, Rendered],
        ),
        (
            DiffTarget::Markdown,
            false,
            Some(LandedMedia::Svg),
            vec![Source, Rendered],
        ),
        (
            DiffTarget::Markdown,
            false,
            Some(LandedMedia::NotPicture),
            vec![Source, Rendered],
        ),
        (DiffTarget::Text, true, None, vec![Source, Preview]),
        (
            DiffTarget::Text,
            true,
            Some(LandedMedia::Picture),
            vec![Source, Preview],
        ),
        (
            DiffTarget::Text,
            true,
            Some(LandedMedia::Svg),
            vec![Source, Preview],
        ),
        (
            DiffTarget::Text,
            true,
            Some(LandedMedia::NotPicture),
            vec![Source, Preview],
        ),
        (DiffTarget::Text, false, None, vec![Source]),
        (
            DiffTarget::Text,
            false,
            Some(LandedMedia::Picture),
            vec![Source],
        ),
        (
            DiffTarget::Text,
            false,
            Some(LandedMedia::Svg),
            vec![Source],
        ),
        (
            DiffTarget::Text,
            false,
            Some(LandedMedia::NotPicture),
            vec![Source],
        ),
        (DiffTarget::Picture, true, None, vec![Rendered, Preview]),
        (
            DiffTarget::Picture,
            true,
            Some(LandedMedia::Picture),
            vec![Rendered, Preview],
        ),
        (
            DiffTarget::Picture,
            true,
            Some(LandedMedia::Svg),
            vec![Rendered, Preview],
        ),
        (
            DiffTarget::Picture,
            true,
            Some(LandedMedia::NotPicture),
            vec![Rendered, Preview],
        ),
        (DiffTarget::Picture, false, None, vec![Rendered]),
        (
            DiffTarget::Picture,
            false,
            Some(LandedMedia::Picture),
            vec![Rendered],
        ),
        (
            DiffTarget::Picture,
            false,
            Some(LandedMedia::Svg),
            vec![Rendered],
        ),
        (
            DiffTarget::Picture,
            false,
            Some(LandedMedia::NotPicture),
            vec![Rendered],
        ),
        (DiffTarget::Svg, true, None, vec![Source, Rendered, Preview]),
        (
            DiffTarget::Svg,
            true,
            Some(LandedMedia::Picture),
            vec![Source, Rendered, Preview],
        ),
        (
            DiffTarget::Svg,
            true,
            Some(LandedMedia::Svg),
            vec![Source, Rendered, Preview],
        ),
        (
            DiffTarget::Svg,
            true,
            Some(LandedMedia::NotPicture),
            vec![Source, Rendered, Preview],
        ),
        (DiffTarget::Svg, false, None, vec![Source, Rendered]),
        (
            DiffTarget::Svg,
            false,
            Some(LandedMedia::Picture),
            vec![Source, Rendered],
        ),
        (
            DiffTarget::Svg,
            false,
            Some(LandedMedia::Svg),
            vec![Source, Rendered],
        ),
        (
            DiffTarget::Svg,
            false,
            Some(LandedMedia::NotPicture),
            vec![Source, Rendered],
        ),
        (DiffTarget::Other, true, None, vec![Source]),
        (
            DiffTarget::Other,
            true,
            Some(LandedMedia::Picture),
            vec![Source],
        ),
        (
            DiffTarget::Other,
            true,
            Some(LandedMedia::Svg),
            vec![Source],
        ),
        (
            DiffTarget::Other,
            true,
            Some(LandedMedia::NotPicture),
            vec![Source],
        ),
        (DiffTarget::Other, false, None, vec![Rendered]),
        (
            DiffTarget::Other,
            false,
            Some(LandedMedia::Picture),
            vec![Rendered],
        ),
        (
            DiffTarget::Other,
            false,
            Some(LandedMedia::Svg),
            vec![Source, Rendered],
        ),
        (
            DiffTarget::Other,
            false,
            Some(LandedMedia::NotPicture),
            vec![Source],
        ),
    ]
}

#[test]
fn diff_representations_decision_table_covers_all_40_combinations() {
    let rows = representation_rows();
    assert_eq!(rows.len(), 40);
    let mut seen = Vec::new();
    for (target, exists, landed, expected) in rows {
        let key = (target, exists, landed);
        assert!(!seen.contains(&key), "duplicate row: {key:?}");
        seen.push(key);
        assert_eq!(
            diff_representations(target, exists, landed),
            expected,
            "{key:?}"
        );
    }
}

#[test]
fn round_diff_view_table_covers_every_requested_view_and_reachable_list() {
    let mut lists = Vec::new();
    for (target, exists, landed, _) in representation_rows() {
        let reps = diff_representations(target, exists, landed);
        if !lists.contains(&reps) {
            lists.push(reps);
        }
    }
    assert_eq!(lists.len(), 6);

    // Expected results for requests in Source, Rendered, Preview order.
    let rows: &[(&[DiffView], [DiffView; 3])] = &[
        (&[Source, Rendered, Preview], [Source, Rendered, Preview]),
        (&[Source, Preview], [Source, Source, Preview]),
        (&[Rendered, Preview], [Rendered, Rendered, Preview]),
        (&[Source], [Source, Source, Source]),
        (&[Rendered], [Rendered, Rendered, Rendered]),
        (&[Source, Rendered], [Source, Rendered, Source]),
    ];
    assert_eq!(rows.len(), lists.len());
    for reps in lists {
        let matching: Vec<_> = rows.iter().filter(|(list, _)| *list == reps).collect();
        assert_eq!(matching.len(), 1, "{reps:?}");
        for (view, expected) in [Source, Rendered, Preview].into_iter().zip(matching[0].1) {
            assert_eq!(
                round_diff_view(view, &reps),
                expected,
                "{view:?} in {reps:?}"
            );
        }
    }
}
