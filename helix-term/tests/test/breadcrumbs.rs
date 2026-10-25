use helix_term::application::Application;
use helix_view::current_ref;

use super::*;

fn assert_cursor_in_text_area(app: &Application) {
    let (view, doc) = current_ref!(app.editor);
    let inner = view.inner_area(doc);
    // The header takes the first row of the view.
    assert_eq!(view.area.top() + 1, inner.top());
    let row = app.editor.cursor().0.expect("cursor should be visible").row as u16;
    assert!(
        (inner.top()..inner.bottom()).contains(&row),
        "cursor row {row} outside of text area {inner:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn breadcrumbs_header_keeps_cursor_in_text_area() -> anyhow::Result<()> {
    let contents: String = (0..200).map(|i| format!("line {i}\n")).collect();
    let file = temp_file_with_contents(contents)?;
    let mut config = test_config();
    config.editor.breadcrumbs = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(file.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            (Some("ge"), Some(&assert_cursor_in_text_area)),
            (Some("gg"), Some(&assert_cursor_in_text_area)),
        ],
        false,
    )
    .await?;

    Ok(())
}
