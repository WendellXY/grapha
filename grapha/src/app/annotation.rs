use crate::AnnotationCommands;

pub(crate) fn handle_annotation_command(command: AnnotationCommands) -> anyhow::Result<()> {
    match command {
        AnnotationCommands::Serve { path, port, watch } => {
            crate::app::serve::handle_serve(path, port, false, watch)
        }
        AnnotationCommands::Sync { server, path } => {
            let report = crate::annotation_sync::sync_annotations(&path, &server)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        AnnotationCommands::List { path } => {
            let records =
                crate::annotations::AnnotationStore::for_project_root(&path).list_records()?;
            let total = records.len();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "project": crate::data_paths::project_identity(&path),
                    "annotations": records,
                    "total": total
                }))?
            );
            Ok(())
        }
    }
}
