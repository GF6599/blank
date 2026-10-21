use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use blank::engine::{DEFAULT_JOBS, Options};
use blank::guard::Guard;
use blank::plan::Plan;
use blank::review::{Answer, Review};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn plan_of(paths: &[&Path]) -> Plan {
    let paths: Vec<PathBuf> = paths.iter().map(|path| path.to_path_buf()).collect();
    Plan::enumerate(&paths, &Guard::for_host()).unwrap()
}

fn review(plan: &Plan) -> Review<'_> {
    let options = Options {
        jobs: DEFAULT_JOBS,
        passes: 2,
        allow_hard_links: false,
    };
    let mut review = Review::new(plan, Arc::new(Guard::for_host()), &options);
    settle(&mut review);
    review
}

fn settle(review: &mut Review<'_>) {
    if let Some(request) = review.work() {
        review.finished(request.run());
    }
}

fn press(review: &mut Review<'_>, code: KeyCode) -> Answer {
    let answer = review.key(KeyEvent::new(code, KeyModifiers::NONE));
    settle(review);
    answer
}

fn typed(review: &mut Review<'_>, text: &str) -> Answer {
    let mut answer = Answer::Waiting;
    for character in text.chars() {
        answer = press(review, KeyCode::Char(character));
    }
    answer
}

fn render(review: &mut Review<'_>, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| review.draw(frame)).unwrap();
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

fn screen_text(review: &mut Review<'_>) -> String {
    lines(&render(review, 100, 30)).join("\n")
}

fn small_tree() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("notes.txt"), "payload").unwrap();
    root
}

/// Only a whole yes, typed and entered, lets the run go on.
///
/// Given: a review of a plan with one file.
/// When: enter is pressed alone, after a part of yes, after yes with a letter too many, after
/// yes in capitals, and after yes.
/// Then: only the whole yes in either case answers destroy, and every other answer waits.
///
/// Why it matters: this screen is the last gate before a run that cannot be undone, so a
/// stray key, a held key, or a slip must never reach it.
#[test]
fn only_a_whole_yes_lets_the_run_go_on() {
    let root = small_tree();
    let plan = plan_of(&[root.path()]);
    for (keys, expected) in [
        ("", Answer::Waiting),
        ("ye", Answer::Waiting),
        ("yess", Answer::Waiting),
        ("YES", Answer::Destroy),
        ("yes", Answer::Destroy),
    ] {
        // Given:
        let mut screen = review(&plan);

        // When:
        typed(&mut screen, keys);
        let answer = press(&mut screen, KeyCode::Enter);

        // Then:
        assert_eq!(answer, expected, "{keys:?}");
    }
}

/// A letter that is not part of yes never reaches the answer.
///
/// Given: a review with the answer empty.
/// When: the operator types y, then a letter that moves the cursor, then e and s, and enter.
/// Then: the answer is yes and the run may go on, and the cursor moved.
///
/// Why it matters: the keys that move through the tree share the keyboard with the answer, and
/// a key that does both turns browsing into typing.
#[test]
fn a_letter_that_is_not_part_of_yes_never_reaches_the_answer() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "alpha").unwrap();
    fs::write(root.path().join("b.txt"), "bravo").unwrap();
    let plan = plan_of(&[root.path()]);
    let mut screen = review(&plan);

    // When:
    typed(&mut screen, "y");
    press(&mut screen, KeyCode::Char('j'));
    typed(&mut screen, "es");
    let answer = press(&mut screen, KeyCode::Enter);

    // Then:
    assert_eq!(answer, Answer::Destroy);
    assert!(screen_text(&mut screen).contains("alpha"));
}

/// Every key that leaves the review destroys nothing.
///
/// Given: a review, once with a part of yes typed.
/// When: q, escape twice, and ctrl+c are pressed.
/// Then: each answers quit, and escape first clears a typed answer.
///
/// Why it matters: leaving is how an operator backs out of a plan aimed at the wrong tree, and
/// it must never be read as consent.
#[test]
fn every_key_that_leaves_the_review_destroys_nothing() {
    let root = small_tree();
    let plan = plan_of(&[root.path()]);

    // When:
    let q = press(&mut review(&plan), KeyCode::Char('q'));
    let mut typing = review(&plan);
    typed(&mut typing, "ye");
    let first_escape = press(&mut typing, KeyCode::Esc);
    let second_escape = press(&mut typing, KeyCode::Esc);
    let mut interrupted = review(&plan);
    typed(&mut interrupted, "yes");
    let control_c = interrupted.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));

    // Then:
    assert_eq!(q, Answer::Quit);
    assert_eq!(first_escape, Answer::Waiting);
    assert_eq!(second_escape, Answer::Quit);
    assert_eq!(control_c, Answer::Quit);
}

/// The review lists every target as a tree with its size, and marks what will survive.
///
/// Given: a tree with a file of 1536 bytes in a subdirectory, a symlink, and a directory nobody
/// can list.
/// When: the review is drawn.
/// Then: the root reads as its full path, the files and the subdirectory are indented under it
/// with their sizes, the subdirectory carries the size of what it holds, and the unreadable
/// directory carries a `!`.
///
/// Why it matters: the operator decides from this screen, and a target missing from it or a
/// survivor shown as destroyed is a decision made on a false picture.
#[test]
fn the_review_lists_every_target_as_a_tree_with_its_size() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let sub = root.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("data.bin"), vec![0; 1536]).unwrap();
    symlink("sub", root.path().join("link")).unwrap();
    let locked = root.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let plan = plan_of(&[root.path()]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    let mut screen = review(&plan);

    // When:
    let text = screen_text(&mut screen);

    // Then:
    let name = root.path().file_name().unwrap().to_str().unwrap();
    assert!(text.contains(name), "{text}");
    let column = |needle: &str| {
        text.lines()
            .find_map(|line| line.find(needle).map(|byte| line[..byte].chars().count()))
            .unwrap_or_else(|| panic!("{needle}:\n{text}"))
    };
    assert_eq!(column("link@"), column("locked/"));
    assert_eq!(column("sub/"), column("link@"));
    assert_eq!(column("data.bin"), column("sub/") + 2);
    let row_of = |needle: &str| {
        text.lines()
            .find(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle}:\n{text}"))
            .to_owned()
    };
    assert!(row_of("data.bin").contains("1.5 KiB"), "{text}");
    assert!(row_of("sub/").contains("1.5 KiB"), "{text}");
    assert!(row_of("locked/").contains('!'), "{text}");
}

/// The review says what the run will destroy, what it will write, and what will survive.
///
/// Given: a plan with one file of 1536 bytes and two random passes.
/// When: the review is drawn.
/// Then: it counts the kinds, and it says what the files hold and what the covers will write.
///
/// Why it matters: the totals are the first sign that a plan is aimed at a tree far larger or
/// smaller than the operator meant.
#[test]
fn the_review_says_what_the_run_will_destroy_and_write() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("data.bin"), vec![0; 1536]).unwrap();
    let plan = plan_of(&[root.path()]);
    let mut screen = review(&plan);

    // When:
    let text = screen_text(&mut screen);

    // Then:
    assert!(text.contains("1 file"), "{text}");
    assert!(text.contains("1.5 KiB"), "{text}");
    assert!(text.contains("4.5 KiB"), "{text}");
}

/// The preview follows the cursor through the tree.
///
/// Given: a review of two text files.
/// When: the cursor moves from the first file to the second.
/// Then: the pane shows the second file's text and no longer the first's.
///
/// Why it matters: the preview is how the operator checks the contents of what the run is about
/// to destroy, and a pane that lags the cursor checks the wrong file.
#[test]
fn the_preview_follows_the_cursor_through_the_tree() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "alpha contents").unwrap();
    fs::write(root.path().join("b.txt"), "bravo contents").unwrap();
    let plan = plan_of(&[root.path()]);
    let mut screen = review(&plan);
    press(&mut screen, KeyCode::Down);

    // When:
    press(&mut screen, KeyCode::Down);

    // Then:
    let text = screen_text(&mut screen);
    assert!(
        text.contains("bravo contents") && !text.contains("alpha contents"),
        "{text}"
    );
}

/// The review draws on any terminal size without failing.
///
/// Given: a review of a small tree with a part of yes typed.
/// When: it is drawn on every terminal from 1 by 1 to 30 by 30.
/// Then: every draw returns.
///
/// Why it matters: a draw that fails ends the screen with the answer half given, and the
/// terminal can be resized to any size while the screen is up.
#[test]
fn the_review_draws_on_any_terminal_size() {
    // Given:
    let root = small_tree();
    let plan = plan_of(&[root.path()]);
    let mut screen = review(&plan);
    typed(&mut screen, "ye");

    for width in 1..=30 {
        for height in 1..=30 {
            // When:
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

            // Then:
            terminal.draw(|frame| screen.draw(frame)).unwrap();
        }
    }
}

/// A small project with a nested file, a symlink and a text file, with yes half typed and the
/// cursor on the text file.
fn project() -> (tempfile::TempDir, Plan) {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir_all(project.join("keys")).unwrap();
    fs::write(project.join("keys").join("id_ed25519"), vec![b'k'; 411]).unwrap();
    fs::write(
        project.join("notes.txt"),
        "rotate the signing key\nmove the backups\n",
    )
    .unwrap();
    symlink("notes.txt", project.join("latest")).unwrap();
    let plan = plan_of(&[&project]);
    (root, plan)
}

/// Compares a render with a reviewed snapshot. The root row holds the path of a temporary
/// directory, which differs on every machine, so it is checked for the project name and size.
fn assert_snapshot(screen: &Buffer, expected: &[&str]) {
    let mut shown = lines(screen);
    while shown.last().is_some_and(String::is_empty) {
        shown.pop();
    }
    assert_eq!(shown.len(), expected.len(), "{}", shown.join("\n"));
    for (line, expected) in shown.iter().zip(expected) {
        if *expected == "ROOT" {
            assert!(
                line.contains("/project/") && line.contains("451 bytes"),
                "{line}"
            );
        } else {
            assert_eq!(line, expected);
        }
    }
}

fn reviewed_project(plan: &Plan) -> Review<'_> {
    let mut screen = review(plan);
    for _ in 0..4 {
        press(&mut screen, KeyCode::Down);
    }
    typed(&mut screen, "ye");
    screen
}

/// A review at a phone width matches the reviewed snapshot.
///
/// Given: a small project with yes half typed and the cursor on a text file.
/// When: it is drawn on a terminal of 40 by 24.
/// Then: every row matches the snapshot reviewed when the layout was set.
///
/// Why it matters: the layout is cell exact, and a change of one cell in a rule or a gap is
/// invisible in a review of the code and obvious on the screen.
#[test]
fn a_review_matches_its_snapshot_at_a_phone_width() {
    // Given:
    let (_root, plan) = project();
    let mut screen = reviewed_project(&plan);

    // When:
    let rendered = render(&mut screen, 40, 24);

    // Then:
    let expected = [
        "",
        "  blank",
        "",
        "  blank will destroy 2 files, 1",
        "  symlink or special file, and 2",
        "  directories.",
        "  The files hold 451 bytes, and the",
        "  covers will write 1.3 KiB (1353",
        "  bytes) in 3 passes.",
        "  Type yes, then enter: ye    [q] quit",
        "",
        "  ┌─ targets ────────────────────────┐",
        "ROOT",
        "  │       keys/         411 bytes    │",
        "  │         id_ed25519  411 bytes    │",
        "  │       latest@                    │",
        "  │ [     notes.txt      40 bytes ]  │",
        "  └──────────────────────────────────┘",
        "  ┌─ preview ────────────────────────┐",
        "  │ rotate the signing key           │",
        "  │ move the backups                 │",
        "  │                                  │",
        "  │                                  │",
        "  └──────────────────────────────────┘",
    ];
    assert_snapshot(&rendered, &expected);
}

/// A review at a desktop width matches the reviewed snapshot.
///
/// Given: a small project with yes half typed and the cursor on a text file.
/// When: it is drawn on a terminal of 100 by 30.
/// Then: every row matches the snapshot reviewed when the layout was set.
///
/// Why it matters: the layout is cell exact, and a change of one cell in a rule or a gap is
/// invisible in a review of the code and obvious on the screen.
#[test]
fn a_review_matches_its_snapshot_at_a_desktop_width() {
    // Given:
    let (_root, plan) = project();
    let mut screen = reviewed_project(&plan);

    // When:
    let rendered = render(&mut screen, 100, 30);

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
        "  blank will destroy 2 files, 1 symlink or special file, and 2 directories.",
        "  The files hold 451 bytes, and the covers will write 1.3 KiB (1353 bytes) in 3",
        "  passes.",
        "  Type yes and press enter to destroy them: ye                 [jk] move  [q] quit",
        "",
        "  ┌─ targets ────────────────────────────┐ ┌─ preview ───────────────────────────┐",
        "ROOT",
        "  │       keys/             411 bytes    │ │ move the backups                    │",
        "  │         id_ed25519      411 bytes    │ │                                     │",
        "  │       latest@                        │ │                                     │",
        "  │ [     notes.txt          40 bytes ]  │ │                                     │",
        "  │                                      │ │                                     │",
        "  │                                      │ │                                     │",
        "  │                                      │ │                                     │",
        "  │                                      │ │                                     │",
        "  │                                      │ │                                     │",
        "  │                                      │ │                                     │",
        "  │                                      │ │                                     │",
        "  │                                      │ │                                     │",
        "  │                                      │ │                                     │",
        "  │                                      │ │                                     │",
        "  └──────────────────────────────────────┘ └─────────────────────────────────────┘",
    ];
    assert_snapshot(&rendered, &expected);
}

/// Each sentence of the header shows whole or not at all.
///
/// Given: a small project.
/// When: the review is drawn at every width from 12 to 100 columns.
/// Then: wherever the totals sentence starts, it also ends on the screen.
///
/// Why it matters: a sentence cut where the header ends drops what the covers will write, which
/// is the number that says how long the disk will be busy.
#[test]
fn each_sentence_of_the_header_shows_whole_or_not_at_all() {
    // Given:
    let (_root, plan) = project();
    let mut screen = review(&plan);

    for width in 12..=100 {
        // When:
        let text = lines(&render(&mut screen, width, 30)).join(" ");

        // Then:
        if text.contains("The files hold") {
            assert!(text.contains("passes."), "{width}: {text}");
        }
    }
}

/// A taller screen shows again the rows a short one scrolled away.
///
/// Given: a review drawn on a short terminal with the cursor near the end of the tree.
/// When: it is drawn on a tall one.
/// Then: the root row is on the screen again.
///
/// Why it matters: a terminal is resized while the screen is up, and a tree that keeps its top
/// rows hidden above empty space hides which root the targets belong to.
#[test]
fn a_taller_screen_shows_again_the_rows_a_short_one_scrolled_away() {
    // Given:
    let (_root, plan) = project();
    let mut screen = review(&plan);
    for _ in 0..4 {
        press(&mut screen, KeyCode::Down);
    }
    let short = lines(&render(&mut screen, 40, 16)).join("\n");
    assert!(
        !short.contains("/project/"),
        "the short screen did not scroll:\n{short}"
    );

    // When:
    let text = lines(&render(&mut screen, 100, 30)).join("\n");

    // Then:
    assert!(text.contains("/project/"), "{text}");
}

/// A file with a hard link outside the selection is marked on the review.
///
/// Given: a tree holding a file that has a second hard link outside it.
/// When: the review is drawn without permission for hard links, and then with it.
/// Then: without it the row carries a `!` and says it is hard-linked, and with it the row
/// keeps the note and loses the `!`.
///
/// Why it matters: the operator cannot see the other names of a file, and the review is where
/// they decide, so it must say which files the run leaves and which it destroys under every name.
#[test]
fn a_file_linked_outside_the_selection_is_marked() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(root.path().join("shared.txt"), "contents").unwrap();
    fs::hard_link(
        root.path().join("shared.txt"),
        outside.path().join("kept.txt"),
    )
    .unwrap();
    let plan = plan_of(&[root.path()]);
    let row = |allow_hard_links: bool| {
        let options = Options {
            jobs: DEFAULT_JOBS,
            passes: 2,
            allow_hard_links,
        };
        let mut screen = Review::new(&plan, Arc::new(Guard::for_host()), &options);
        lines(&render(&mut screen, 100, 30))
            .into_iter()
            .find(|line| line.contains("shared.txt"))
            .expect("the file has a row")
    };

    // When:
    let refused = row(false);
    let allowed = row(true);

    // Then:
    assert!(
        refused.contains('!') && refused.contains("hard-linked"),
        "{refused}"
    );
    assert!(
        !allowed.contains('!') && allowed.contains("hard-linked"),
        "{allowed}"
    );
}
