use helix_term::application::Application;
use helix_view::{current_ref, doc, view};

use super::*;

fn app(lines: usize) -> anyhow::Result<(Application, tempfile::NamedTempFile)> {
    let file = temp_file_with_contents("line\n".repeat(lines))?;
    let mut config = test_config();
    config.editor.scrollbar = true;
    let app = AppBuilder::new()
        .with_config(config)
        .with_file(file.path(), None)
        .build()?;
    Ok((app, file))
}

fn top_line(app: &Application) -> usize {
    let (view, doc) = current_ref!(app.editor);
    doc.text().char_to_line(doc.view_offset(view.id).anchor)
}

fn cursor_line(app: &Application) -> usize {
    let (view, doc) = current_ref!(app.editor);
    let text = doc.text().slice(..);
    text.char_to_line(doc.selection(view.id).primary().cursor(text))
}

#[tokio::test(flavor = "multi_thread")]
async fn takes_the_last_column() -> anyhow::Result<()> {
    let (app, _file) = app(500)?;
    let (view, doc) = current_ref!(app.editor);
    let bar = view.scrollbar_area().unwrap();
    assert_eq!(view.area.right() - 1, bar.x);
    assert_eq!(bar.x, view.inner_area(doc).right());
    assert_eq!(view.inner_height(), bar.height as usize);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn thumb_follows_the_view() -> anyhow::Result<()> {
    let (mut app, _file) = app(500)?;
    let thumb = |app: &Application| {
        let (view, doc) = current_ref!(app.editor);
        (
            view.scrollbar_thumb(doc).unwrap(),
            view.scrollbar_area().unwrap().height,
        )
    };
    test_key_sequences(
        &mut app,
        vec![
            (
                None,
                Some(&|app| {
                    let (thumb, height) = thumb(app);
                    assert_eq!(0, thumb.start);
                    // The view shows about a quarter of the file.
                    let expected = (height as usize).pow(2).div_ceil(500) as u16;
                    assert_eq!(expected, thumb.end);
                }),
            ),
            (
                Some("ge"),
                Some(&|app| {
                    let (thumb, height) = thumb(app);
                    assert_eq!(height, thumb.end);
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn no_thumb_when_everything_is_visible() -> anyhow::Result<()> {
    let (app, _file) = app(10)?;
    let (view, doc) = current_ref!(app.editor);
    assert!(view.scrollbar_area().is_some());
    assert_eq!(None, view.scrollbar_thumb(doc));
    Ok(())
}

#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread")]
async fn dragging_the_thumb_scrolls() -> anyhow::Result<()> {
    use termina::event::{Event, Modifiers, MouseButton, MouseEvent, MouseEventKind};
    use tokio_stream::wrappers::UnboundedReceiverStream;

    let (mut app, _file) = app(500)?;
    let bar = view!(app.editor).scrollbar_area().unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut events = UnboundedReceiverStream::new(rx);
    let mouse = |kind, row| {
        Ok(Event::Mouse(MouseEvent {
            kind,
            column: bar.x,
            row,
            modifiers: Modifiers::NONE,
        }))
    };
    let height = bar.height as usize;
    let max_top = 500 - height;

    // Clicking the middle of the track centers the thumb there.
    tx.send(mouse(
        MouseEventKind::Down(MouseButton::Left),
        bar.y + bar.height / 2,
    ))?;
    app.event_loop_until_idle(&mut events).await;
    let middle = top_line(&app);
    assert!(
        middle.abs_diff(max_top / 2) <= 2,
        "{middle} should be about {}",
        max_top / 2
    );
    // The cursor is pulled into the view.
    assert!(cursor_line(&app) >= middle && cursor_line(&app) < middle + height);

    // Dragging to the bottom shows the end of the file.
    tx.send(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        bar.bottom() - 1,
    ))?;
    app.event_loop_until_idle(&mut events).await;
    assert_eq!(max_top, top_line(&app));

    // Dragging beyond the top of the scrollbar shows the start.
    tx.send(mouse(MouseEventKind::Drag(MouseButton::Left), 0))?;
    tx.send(mouse(MouseEventKind::Up(MouseButton::Left), 0))?;
    app.event_loop_until_idle(&mut events).await;
    assert_eq!(0, top_line(&app));
    assert!(cursor_line(&app) < height);
    assert!(!doc!(app.editor).is_modified());

    Ok(())
}
