use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use blank::engine::{DEFAULT_JOBS, Options, Stage, destroy};
use blank::guard::Guard;
use blank::plan::{Incomplete, Plan};
use blank::view::{Action, Outcome, RunView};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;

const GOLD: Color = Color::Rgb(0xf6, 0xc1, 0x77);
const IRIS: Color = Color::Rgb(0xc4, 0xa7, 0xe7);
const LOVE: Color = Color::Rgb(0xeb, 0x6f, 0x92);

fn render(view: &mut RunView, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| view.draw(frame)).unwrap();
    terminal.backend().buffer().clone()
}

fn lines(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// The position of `text` on the screen, as a column and a row.
fn find(buffer: &Buffer, text: &str) -> (u16, u16) {
    let found = lines(buffer).iter().enumerate().find_map(|(y, line)| {
        line.find(text).map(|byte| {
            let column = line[..byte].chars().count();
            (column as u16, y as u16)
        })
    });
    found.unwrap_or_else(|| {
        panic!(
            "{text:?} is not on the screen:\n{}",
            lines(buffer).join("\n")
        )
    })
}

fn column_of_first(buffer: &Buffer, row: u16, symbol: char) -> u16 {
    let line: Vec<char> = lines(buffer)[row as usize].chars().collect();
    line.iter()
        .position(|&cell| cell == symbol)
        .expect("the symbol is on the row") as u16
}

fn column_of_last(buffer: &Buffer, row: u16, symbol: char) -> u16 {
    let line: Vec<char> = lines(buffer)[row as usize].chars().collect();
    line.iter()
        .rposition(|&cell| cell == symbol)
        .expect("the symbol is on the row") as u16
}

/// The files the panel shows, top to bottom.
fn files_in_view(buffer: &Buffer) -> Vec<String> {
    lines(buffer)
        .iter()
        .filter_map(|line| line.split("/data/archive/").nth(1))
        .map(|rest| {
            rest.split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(view: &mut RunView, code: KeyCode) -> Action {
    view.key(key(code))
}

fn typed(view: &mut RunView, text: &str) {
    for character in text.chars() {
        press(view, KeyCode::Char(character));
    }
}

fn path(name: &str) -> PathBuf {
    Path::new("/data/archive").join(name)
}

fn started(view: &mut RunView, name: &str) {
    view.target("Overwriting", &path(name), Stage::Start);
}

fn settled(view: &mut RunView, name: &str, stage: Stage) {
    view.target("Overwriting", &path(name), stage);
}

/// The report of a run that could not destroy one target, because its argument does not
/// exist.
fn one_survivor() -> Result<(), Incomplete> {
    let missing = tempfile::tempdir().unwrap().path().join("missing.txt");
    let plan = Plan::enumerate(&[missing], &Guard::for_host()).unwrap();
    let options = Options {
        jobs: DEFAULT_JOBS,
        passes: 1,
        allow_hard_links: false,
    };
    destroy(&plan, options, &AtomicBool::new(false), &mut |_| {})
}

/// The status line says what the run reached, and its words match the exit code.
///
/// Given: a run in progress, then the same run finished clean, finished with one survivor,
/// and stopped by the operator.
/// When: each is drawn.
/// Then: the status line reads blanking, blanking complete, blanking failed with the count, and
/// blanking cancelled, and the verdict printed after exit uses the same words.
///
/// Why it matters: the interface and the exit code are two reports of one run, and a status
/// line that says complete above a failure teaches the operator to ignore the exit code.
#[test]
fn the_status_line_and_the_verdict_report_the_same_outcome() {
    // Given:
    let mut active = RunView::new();
    let mut clean = RunView::new();
    clean.finish(&Ok(()));
    let mut failed = RunView::new();
    failed.finish(&one_survivor());
    let mut stopped = RunView::new();
    press(&mut stopped, KeyCode::Char('q'));
    stopped.finish(&one_survivor());

    // When:
    let cases = [
        (&mut active, "blanking", Outcome::Active),
        (&mut clean, "blanking complete", Outcome::Complete),
        (
            &mut failed,
            "blanking failed, 1 target survived",
            Outcome::Failed { survivors: 1 },
        ),
        (&mut stopped, "blanking cancelled", Outcome::Cancelled),
    ];

    // Then:
    for (view, words, outcome) in cases {
        assert_eq!(view.outcome(), outcome);
        let screen = render(view, 80, 24);
        assert!(
            lines(&screen).iter().any(|line| line.trim() == words),
            "{words}"
        );
        if outcome != Outcome::Active {
            assert_eq!(outcome.verdict(), words);
        }
    }
}

/// The status line fits the column at every width, and still names the outcome.
///
/// Given: a run that left two survivors.
/// When: it is drawn at every width from 12 to 90 columns.
/// Then: the status line never runs past the right edge, and it always says failed.
///
/// Why it matters: a line cut to fit can drop the word that carries the outcome, and a line
/// that overruns the column is cut by the terminal at a place nobody chose.
#[test]
fn the_status_line_fits_every_width_and_keeps_the_outcome() {
    // Given:
    let missing = tempfile::tempdir().unwrap();
    let args = [missing.path().join("a"), missing.path().join("b")];
    let plan = Plan::enumerate(&args, &Guard::for_host()).unwrap();
    let options = Options {
        jobs: DEFAULT_JOBS,
        passes: 1,
        allow_hard_links: false,
    };
    let report = destroy(&plan, options, &AtomicBool::new(false), &mut |_| {});
    let mut view = RunView::new();
    view.finish(&report);

    for width in 12..=90 {
        // When:
        let screen = render(&mut view, width, 24);

        // Then:
        let (_, row) = find(&screen, "failed");
        let line = &lines(&screen)[row as usize];
        assert!(
            line.chars().count() <= width as usize - 2,
            "{width}: {line:?}"
        );
    }
}

/// The target in flight carries a gold bracket pair and an iris label, and a survivor a red
/// mark.
///
/// Given: a run with one file destroyed, one failed, and one in flight.
/// When: it is drawn.
/// Then: only the newest row is bracketed, its brackets are gold and its label iris, and the
/// failed row carries a red `!`.
///
/// Why it matters: the bracket is how the operator finds the work in flight among hundreds of
/// rows, and the `!` is the one row they must not skim past.
#[test]
fn the_row_in_flight_is_bracketed_and_a_survivor_is_marked() {
    // Given:
    let mut view = RunView::new();
    started(&mut view, "done.txt");
    settled(&mut view, "done.txt", Stage::Done);
    started(&mut view, "failed.txt");
    settled(&mut view, "failed.txt", Stage::Failed);
    started(&mut view, "busy.txt");

    // When:
    let screen = render(&mut view, 80, 24);

    // Then:
    let (label, row) = find(&screen, "Overwriting /data/archive/busy.txt");
    let open = column_of_first(&screen, row, '[');
    assert_eq!(screen[(open, row)].fg, GOLD);
    assert_eq!(screen[(label, row)].fg, IRIS);
    let close = column_of_last(&screen, row, ']');
    assert_eq!(screen[(close, row)].fg, GOLD);
    for other in ["done.txt", "failed.txt"] {
        let (_, row) = find(&screen, other);
        assert!(!lines(&screen)[row as usize].contains('['), "{other}");
    }
    let (_, failed) = find(&screen, "failed.txt");
    let mark = column_of_last(&screen, failed, '!');
    assert_eq!(screen[(mark, failed)].fg, LOVE);
}

/// The filter narrows the panel, and its heading carries the filter and both counts.
///
/// Given: a panel of three rows.
/// When: the operator types a filter that matches one row, in a different letter case.
/// Then: the panel shows only that row, and the heading reads the filter and 1 of 3.
///
/// Why it matters: a filtered panel looks exactly like a short run, so the heading is the only
/// thing that says rows are hidden.
#[test]
fn the_filter_narrows_the_panel_and_says_so_in_the_heading() {
    // Given:
    let mut view = RunView::new();
    for name in ["alpha.txt", "beta.txt", "gamma.txt"] {
        started(&mut view, name);
        settled(&mut view, name, Stage::Done);
    }

    // When:
    press(&mut view, KeyCode::Char('/'));
    typed(&mut view, "BETA");
    press(&mut view, KeyCode::Enter);

    // Then:
    let screen = render(&mut view, 80, 24);
    find(&screen, "activity  BETA  1 of 3");
    find(&screen, "beta.txt");
    let text = lines(&screen).join("\n");
    assert!(
        !text.contains("alpha.txt") && !text.contains("gamma.txt"),
        "{text}"
    );
}

/// Escape clears the filter before it stops the run, and ctrl+c stops it from anywhere.
///
/// Given: a run with a filter set, and another with the filter box open.
/// When: escape is pressed on the first, twice, and ctrl+c on the second.
/// Then: the first escape clears the filter and keeps the run, the second stops it, and ctrl+c
/// stops the run although the box is open.
///
/// Why it matters: this screen is destroying files, so the key that stops it must never become
/// text in an input, and a key meant to clear a filter must not stop a run.
#[test]
fn escape_clears_the_filter_first_and_ctrl_c_always_stops() {
    // Given:
    let mut filtered = RunView::new();
    started(&mut filtered, "alpha.txt");
    press(&mut filtered, KeyCode::Char('/'));
    typed(&mut filtered, "alpha");
    press(&mut filtered, KeyCode::Enter);
    let mut typing = RunView::new();
    press(&mut typing, KeyCode::Char('/'));

    // When:
    let first = press(&mut filtered, KeyCode::Esc);
    let second = press(&mut filtered, KeyCode::Esc);
    let interrupt = typing.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));

    // Then:
    assert_eq!(first, Action::None);
    assert_eq!(second, Action::Stop);
    assert_eq!(interrupt, Action::Stop);
}

/// The panel follows the newest row until the operator scrolls away, and again once they come
/// back.
///
/// Given: a panel holding more rows than it can show.
/// When: the operator scrolls up one row, a row arrives, and then they scroll back to the bottom.
/// Then: the rows that arrived do not move the panel, and after scrolling down it shows the
/// newest row again.
///
/// Why it matters: a panel that jumps to the bottom on every row cannot be read during a run,
/// and one that never follows again hides the work in flight.
#[test]
fn the_panel_holds_where_the_operator_scrolled_and_follows_again_at_the_bottom() {
    // Given:
    let mut view = RunView::new();
    for index in 0..40 {
        started(&mut view, &format!("file-{index:02}.bin"));
        settled(&mut view, &format!("file-{index:02}.bin"), Stage::Done);
    }
    render(&mut view, 80, 24);

    // When:
    press(&mut view, KeyCode::Char('k'));
    let held = files_in_view(&render(&mut view, 80, 24));
    started(&mut view, "file-40.bin");
    let after_arrival = files_in_view(&render(&mut view, 80, 24));
    press(&mut view, KeyCode::PageDown);
    let followed = render(&mut view, 80, 24);

    // Then:
    assert_eq!(held, after_arrival);
    find(&followed, "file-40.bin");
}

/// The panel keeps a bounded history of rows.
///
/// Given: a run of 250 files.
/// When: the operator scrolls to the top of the panel.
/// Then: the oldest row shown is file 50, so the panel holds the newest 200.
///
/// Why it matters: every row is redrawn on every frame, so an unbounded history grows the cost
/// of each frame with the size of the tree, and the manifest already holds every path.
#[test]
fn the_panel_keeps_the_newest_two_hundred_rows() {
    // Given:
    let mut view = RunView::new();
    for index in 0..250 {
        started(&mut view, &format!("file-{index:03}.bin"));
        settled(&mut view, &format!("file-{index:03}.bin"), Stage::Done);
    }

    // When:
    render(&mut view, 80, 24);
    for _ in 0..200 {
        press(&mut view, KeyCode::PageUp);
    }

    // Then:
    let screen = render(&mut view, 80, 24);
    find(&screen, "file-050.bin");
    assert!(!lines(&screen).join("\n").contains("file-049.bin"));
}

/// The panel's top rule keeps a gap after its title, or drops the title, at every width.
///
/// Given: a filtered panel, whose heading is long.
/// When: it is drawn at every width from 12 to 90 columns.
/// Then: the top rule ends above the bottom corner, and a title in it is followed by a space and
/// at least one rule cell.
///
/// Why it matters: a title admitted without room for its gap welds itself to the corner, and a
/// fit check that is one cell short only fails at the widths where the title nearly fits.
#[test]
fn the_panel_title_never_welds_to_the_corner() {
    // Given:
    let mut view = RunView::new();
    started(&mut view, "alpha.txt");
    press(&mut view, KeyCode::Char('/'));
    typed(&mut view, "alpha");
    press(&mut view, KeyCode::Enter);

    for width in 12..=90 {
        // When:
        let screen = render(&mut view, width, 24);

        // Then:
        let (left, row) = find(&screen, "┌");
        let line: Vec<char> = lines(&screen)[row as usize].chars().collect();
        let rule: String = line[left as usize..].iter().collect();
        assert!(rule.ends_with('┐'), "{width}: {rule}");
        let (bottom_right, _) = find(&screen, "┘");
        assert_eq!(line.len() as u16 - 1, bottom_right, "{width}: {rule}");
        if rule.contains("activity") {
            let after = rule
                .split("of 1")
                .nth(1)
                .unwrap_or_else(|| rule.split("activity").nth(1).expect("the title is whole"));
            assert!(after.starts_with(" ─"), "{width}: {rule}");
        } else {
            assert!(
                rule[3..rule.len() - 3].chars().all(|cell| cell == '─'),
                "{width}: {rule}"
            );
        }
    }
}

/// A run with rows in each state, part way through, as the snapshots draw it.
fn mid_run() -> RunView {
    let mut view = RunView::new();
    for (name, stage) in [
        ("reports/q3-summary.pdf", Stage::Done),
        ("keys/id_ed25519", Stage::Failed),
        ("photos/2026/family-reunion-panorama.jpg", Stage::Done),
    ] {
        started(&mut view, name);
        settled(&mut view, name, stage);
    }
    started(&mut view, "notes/passwords.txt");
    view.progress(66, 100);
    view
}

/// A run part way through, at a phone width, matches the reviewed snapshot.
///
/// Given: a run with a destroyed row, a survivor, and a row in flight, two thirds done.
/// When: it is drawn on a terminal of 40 by 20.
/// Then: every row of the screen matches the snapshot reviewed when the layout was set.
///
/// Why it matters: the layout is cell exact, and a change of one cell in a rule or a gap is
/// invisible in a review of the code and obvious on the screen.
#[test]
fn a_run_matches_its_snapshot_at_a_phone_width() {
    // Given:
    let mut view = mid_run();

    // When:
    let screen = render(&mut view, 40, 20);

    // Then:
    let expected = [
        "",
        "  blank",
        "",
        "  █████░░░  66%  [jk] scroll  [q] quit",
        "  blanking",
        "",
        "  ┌─ activity ───────────────────────┐",
        "  │     ...reports/q3-summary.pdf    │",
        "  │   ! ...rchive/keys/id_ed25519    │",
        "  │     ...y-reunion-panorama.jpg    │",
        "  │ [ ⠋ ...ve/notes/passwords.txt ]  │",
        "  │                                  │",
        "  │                                  │",
        "  │                                  │",
        "  │                                  │",
        "  │                                  │",
        "  │                                  │",
        "  │                                  │",
        "  │                                  │",
        "  └──────────────────────────────────┘",
    ];
    let mut shown = lines(&screen);
    while shown.last().is_some_and(String::is_empty) {
        shown.pop();
    }
    assert_eq!(shown, expected);
}

/// A run part way through, at a desktop width, matches the reviewed snapshot.
///
/// Given: a run with a destroyed row, a survivor, and a row in flight, two thirds done.
/// When: it is drawn on a terminal of 100 by 30.
/// Then: every row of the screen matches the snapshot reviewed when the layout was set.
///
/// Why it matters: the layout is cell exact, and a change of one cell in a rule or a gap is
/// invisible in a review of the code and obvious on the screen.
#[test]
fn a_run_matches_its_snapshot_at_a_desktop_width() {
    // Given:
    let mut view = mid_run();

    // When:
    let screen = render(&mut view, 100, 30);

    // Then:
    let expected = [
        "",
        "  ██████╗ ██╗      █████╗ ███╗   ██╗██╗  ██╗",
        "  ██╔══██╗██║     ██╔══██╗████╗  ██║██║ ██╔╝",
        "  ██████╔╝██║     ███████║██╔██╗ ██║█████╔╝",
        "  ██╔══██╗██║     ██╔══██║██║╚██╗██║██╔═██╗",
        "  ██████╔╝███████╗██║  ██║██║ ╚████║██║  ██╗",
        "  ╚═════╝ ╚══════╝╚═╝  ╚═╝╚═╝  ╚═══╝╚═╝  ╚═╝",
        "",
        "  ████████████████████████████████████░░░░░░░░░░░░░░░░░░░░  66%  [/] filter  [jk] scroll  [q] quit",
        "  blanking",
        "",
        "  ┌─ activity ───────────────────────────────────────────────────────────────────────────────────┐",
        "  │     Overwriting /data/archive/reports/q3-summary.pdf                                         │",
        "  │   ! Overwriting /data/archive/keys/id_ed25519                                                │",
        "  │     Overwriting /data/archive/photos/2026/family-reunion-panorama.jpg                        │",
        "  │ [ ⠋ Overwriting /data/archive/notes/passwords.txt                                         ]  │",
        "  │                                                                                              │",
        "  │                                                                                              │",
        "  │                                                                                              │",
        "  │                                                                                              │",
        "  │                                                                                              │",
        "  │                                                                                              │",
        "  │                                                                                              │",
        "  │                                                                                              │",
        "  │                                                                                              │",
        "  │                                                                                              │",
        "  │                                                                                              │",
        "  │                                                                                              │",
        "  │                                                                                              │",
        "  └──────────────────────────────────────────────────────────────────────────────────────────────┘",
    ];
    let mut shown = lines(&screen);
    while shown.last().is_some_and(String::is_empty) {
        shown.pop();
    }
    assert_eq!(shown, expected);
}

/// The view draws on any terminal size without failing.
///
/// Given: a run part way through, with the filter box open.
/// When: it is drawn on every terminal from 1 by 1 to 24 by 24.
/// Then: every draw returns.
///
/// Why it matters: a draw that fails ends the screen in the middle of a run, and a terminal
/// can be resized to any size while the run goes on.
#[test]
fn the_view_draws_on_any_terminal_size() {
    // Given:
    let mut view = mid_run();
    press(&mut view, KeyCode::Char('/'));
    typed(&mut view, "notes");

    for width in 1..=24 {
        for height in 1..=24 {
            // When:
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

            // Then:
            terminal.draw(|frame| view.draw(frame)).unwrap();
        }
    }
}
