use helix_view::{DocumentId, Editor};

use crate::commands::typed::reload_document;

/// Reload every document whose file was changed on disk by an external process, and refresh
/// the diff base of documents whose repository's HEAD or index changed.
///
/// Documents with unsaved changes are never reloaded; the user is warned once per external
/// change instead. Returns `true` if anything visible changed and a redraw is needed.
///
/// This costs a few `stat` calls per open file and is called from the `auto-reload` timer and
/// when the terminal regains focus.
pub fn reload_externally_modified(editor: &mut Editor) -> bool {
    // Our own in-flight writes bump the mtime before `last_saved_time` is updated.
    if editor.write_count > 0 {
        return false;
    }

    let changed: Vec<(DocumentId, _)> = editor
        .documents()
        .filter_map(|doc| Some((doc.id(), doc.externally_modified_time()?)))
        .collect();

    let mut needs_redraw = false;
    for (doc_id, mtime) in changed {
        let Some(doc) = editor.documents.get_mut(&doc_id) else {
            continue;
        };
        let name = doc.display_name().into_owned();
        if doc.is_modified() {
            if doc.mark_external_change_warned(mtime) {
                editor.set_warning(format!(
                    "{name} changed on disk but has unsaved changes (:reload to discard yours, :w! to overwrite)"
                ));
                needs_redraw = true;
            }
            continue;
        }

        match reload_document(editor, doc_id) {
            Ok(()) => editor.set_status(format!("{name} reloaded (changed on disk)")),
            Err(err) => editor.set_error(format!("failed to reload {name}: {err}")),
        }
        needs_redraw = true;
    }

    // A commit, checkout or reset changes the diff base even if the file itself didn't change.
    let vcs_changed: Vec<DocumentId> = editor
        .documents
        .values_mut()
        .filter_map(|doc| doc.vcs_state_changed().then(|| doc.id()))
        .collect();
    for doc_id in vcs_changed {
        let Some(doc) = editor.documents.get_mut(&doc_id) else {
            continue;
        };
        let trust_full = editor
            .workspace_trust
            .query(
                doc.workspace_root(),
                helix_loader::workspace_trust::TrustQuery::Git,
            )
            .is_trusted();
        doc.refresh_vcs(&editor.diff_providers, trust_full);
        needs_redraw = true;
    }

    needs_redraw
}
