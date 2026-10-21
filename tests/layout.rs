use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use blank::engine::{DEFAULT_JOBS, Options};
use blank::guard::Guard;
use blank::plan::Plan;
use blank::review::Review;
use blank::selector::Selector;
use blank::view::RunView;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

const MARGIN: u16 = 2;

fn render(width: u16, draw: impl FnOnce(&mut Frame<'_>)) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
    terminal.draw(draw).unwrap();
    terminal.backend().buffer().clone()
}

/// The leftmost top-left corner and the rightmost top-right corner on the screen.
fn frame_edges(buffer: &Buffer) -> (u16, u16) {
    let Rect { width, height, .. } = buffer.area;
    let cells = (0..height).flat_map(|y| (0..width).map(move |x| (x, y)));
    let at = |symbol: &str| {
        cells
            .clone()
            .filter(|&(x, y)| buffer[(x, y)].symbol() == symbol)
            .map(|(x, _)| x)
            .collect::<Vec<_>>()
    };
    let left = at("┌").into_iter().min().expect("a frame is drawn");
    let right = at("┐").into_iter().max().expect("a frame is drawn");
    (left, right)
}

/// Every screen spans the terminal, less its margin, however wide the terminal is.
///
/// Given: a run view, a selector, and a review of a small tree.
/// When: each is drawn on a terminal 80 columns wide and on one 200 columns wide.
/// Then: on both, the frames run from the left margin to the right margin.
///
/// Why it matters: a wide terminal is where long paths fit whole, and a column that stops at a
/// fixed width cuts them while the rest of the screen stays empty.
#[test]
fn every_screen_spans_the_terminal_less_its_margin() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("notes.txt"), "hello\n").unwrap();
    let guard = Arc::new(Guard::for_host());
    let plan = Plan::enumerate(&[PathBuf::from(root.path())], &guard).unwrap();
    let options = Options {
        jobs: DEFAULT_JOBS,
        passes: 2,
        allow_hard_links: false,
    };
    let mut view = RunView::new();
    let (mut selector, load) = Selector::open(root.path(), Arc::clone(&guard));
    selector.loaded(load.run());
    let mut review = Review::new(&plan, Arc::clone(&guard), &options);

    for width in [80, 200] {
        // When:
        let screens = [
            ("run view", render(width, |frame| view.draw(frame))),
            ("selector", render(width, |frame| selector.draw(frame))),
            ("review", render(width, |frame| review.draw(frame))),
        ];

        // Then:
        for (name, screen) in screens {
            assert_eq!(
                frame_edges(&screen),
                (MARGIN, width - MARGIN - 1),
                "{name} at {width} columns"
            );
        }
    }
}
