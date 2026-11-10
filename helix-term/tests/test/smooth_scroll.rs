use std::cell::Cell;
use std::time::Duration;

use helix_term::application::Application;
use helix_view::editor::SmoothScroll;
use helix_view::view::ViewPosition;
use helix_view::{doc, view};

use super::*;

fn app(file: &tempfile::NamedTempFile, smooth_scroll: SmoothScroll) -> anyhow::Result<Application> {
    let mut config = test_config();
    config.editor.smooth_scroll = smooth_scroll;
    AppBuilder::new()
        .with_config(config)
        .with_file(file.path(), None)
        .build()
}

async fn view_offset_after(
    keys: &str,
    smooth_scroll: SmoothScroll,
) -> anyhow::Result<ViewPosition> {
    let file = temp_file_with_contents("line\n".repeat(500))?;
    let mut app = app(&file, smooth_scroll)?;
    let offset = Cell::new(ViewPosition::default());
    // Unlike the real event loop, tests don't draw a frame on startup. Animations start from
    // the last drawn frame, so draw one first.
    test_key_sequences(
        &mut app,
        vec![
            (Some("<esc>"), None),
            (
                Some(keys),
                Some(&|app| offset.set(doc!(app.editor).view_offset(view!(app.editor).id))),
            ),
        ],
        false,
    )
    .await?;
    Ok(offset.get())
}

#[tokio::test(flavor = "multi_thread")]
async fn animated_scroll_ends_where_instant_scroll_does() -> anyhow::Result<()> {
    // The second key arrives mid-animation and must start from the first one's target.
    for keys in ["<C-d>", "<C-d><C-d>", "<C-f><C-u>", "ge", "ge<C-b>"] {
        let instant = view_offset_after(keys, SmoothScroll::default()).await?;
        let animated = view_offset_after(
            keys,
            SmoothScroll {
                enable: true,
                // Shorter than the idle timeout, so the animation finishes before the check.
                duration: 50,
            },
        )
        .await?;
        assert_ne!(instant, ViewPosition::default(), "{keys} didn't scroll");
        assert_eq!(instant, animated, "{keys}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn animation_redraws_for_its_duration() -> anyhow::Result<()> {
    let instant = view_offset_after("ge", SmoothScroll::default()).await?;
    // In integration tests every redraw resets the idle timer, so the key sequence only
    // finishes once the animation has stopped requesting frames.
    let duration = Duration::from_millis(1000);
    let start = std::time::Instant::now();
    let animated = view_offset_after(
        "ge",
        SmoothScroll {
            enable: true,
            duration: duration.as_millis() as u64,
        },
    )
    .await?;
    assert!(start.elapsed() >= duration, "{:?}", start.elapsed());
    assert_eq!(instant, animated);
    Ok(())
}
