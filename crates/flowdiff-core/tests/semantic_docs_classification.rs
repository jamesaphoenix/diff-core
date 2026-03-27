use flowdiff_core::cluster::classify_by_convention;
use flowdiff_core::types::InfraCategory;

#[test]
fn semantic_docs_trees_stay_groupable() {
    for path in [
        "docs/tutorial/where.md",
        "documentation/docs/02-runes/03-$derived.md",
        "docs_src/tutorial/where/tutorial006b_py310.py",
        "docs/css/custom.css",
        "docs/plugin-protocol/tfplugin6.proto",
    ] {
        assert_eq!(
            classify_by_convention(path),
            InfraCategory::DirectoryGroup,
            "expected semantic docs tree content to stay groupable: {path}"
        );
    }
}

#[test]
fn docs_meta_and_site_config_stay_non_groupable() {
    assert_eq!(
        classify_by_convention("CHANGELOG.md"),
        InfraCategory::Documentation
    );
    assert_eq!(
        classify_by_convention("docs/README.md"),
        InfraCategory::Documentation
    );
    assert_eq!(
        classify_by_convention("docs/en/mkdocs.yml"),
        InfraCategory::Documentation
    );
}
