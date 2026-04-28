use std::path::PathBuf;

pub(crate) fn handle_migrate(
    path: PathBuf,
    from: Option<PathBuf>,
    force: bool,
) -> anyhow::Result<()> {
    let report = crate::migration::migrate_local_grapha(&path, from.as_deref(), force)?;

    println!(
        "migrated temporary Grapha index from {}",
        report.source_project_root.display()
    );
    println!(
        "  {} nodes, {} edges available in {}",
        report.node_count,
        report.edge_count,
        report.target_store_dir.display()
    );
    if !report.migrated_artifacts.is_empty() {
        println!("  migrated: {}", report.migrated_artifacts.join(", "));
    }
    if !report.skipped_artifacts.is_empty() {
        println!("  preserved: {}", report.skipped_artifacts.join(", "));
    }
    println!(
        "  run `grapha index {}` to replace it",
        report.target_project_root.display()
    );

    Ok(())
}
