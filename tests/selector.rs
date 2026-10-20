use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use blank::guard::Guard;
use blank::selector::{Pick, Selector};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;

const GOLD: Color = Color::Rgb(0xf6, 0xc1, 0x77);
const IRIS: Color = Color::Rgb(0xc4, 0xa7, 0xe7);
const LOVE: Color = Color::Rgb(0xeb, 0x6f, 0x92);

/// Opens a selector on `dir` and finishes its first load, as the screen would.
fn open(dir: &Path) -> Selector {
    let (mut selector, load) = Selector::open(dir, Arc::new(Guard::for_host()));
    selector.loaded(load.run());
    settle(&mut selector);
    selector
}

/// Runs every preview and total the selector asks for, as the screen runs them on threads.
fn settle(selector: &mut Selector) {
    for work in selector.work() {
        selector.finished(work.run());
    }
}

/// Presses a key, and finishes any load and any work it starts.
fn press(selector: &mut Selector, code: KeyCode) -> Pick {
    let pick = selector.key(KeyEvent::new(code, KeyModifiers::NONE));
    if let Pick::Load(load) = &pick {
        selector.loaded(load.run());
    }
    settle(selector);
    pick
}

fn typed(selector: &mut Selector, text: &str) {
    for character in text.chars() {
        press(selector, KeyCode::Char(character));
    }
}

/// Moves the cursor to the first entry whose name holds `name`, through an applied filter.
fn select(selector: &mut Selector, name: &str) {
    press(selector, KeyCode::Char('/'));
    typed(selector, name);
    press(selector, KeyCode::Enter);
}

fn render(selector: &mut Selector, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| selector.draw(frame)).unwrap();
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

fn screen_text(selector: &mut Selector) -> String {
    lines(&render(selector, 80, 24)).join("\n")
}

/// The row of the screen that holds `text`, as a row number and its cells.
fn row_of(buffer: &Buffer, text: &str) -> (u16, Vec<char>) {
    let found = lines(buffer)
        .into_iter()
        .enumerate()
        .find(|(_, line)| line.contains(text));
    let (y, line) = found.unwrap_or_else(|| {
        panic!(
            "{text:?} is not on the screen:\n{}",
            lines(buffer).join("\n")
        )
    });
    (y as u16, line.chars().collect())
}

/// The state cell of the row that holds `text`: the cell two after the bracket column.
fn state_cell(buffer: &Buffer, text: &str) -> (char, Color) {
    let (y, line) = row_of(buffer, text);
    let rule = line
        .iter()
        .position(|&cell| cell == '│')
        .expect("the row is framed");
    let x = rule + 4;
    (line[x], buffer[(x as u16, y)].fg)
}

fn tree(names: &[&str]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for name in names {
        let path = root.path().join(name);
        if let Some(dir) = name.strip_suffix('/') {
            fs::create_dir_all(root.path().join(dir)).unwrap();
        } else {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "payload").unwrap();
        }
    }
    root
}

/// A path the run would refuse cannot be marked, and the screen says why.
///
/// Given: a selector at the root of the filesystem, with the cursor on `usr`.
/// When: space is pressed.
/// Then: nothing is marked, and the status row carries the refusal.
///
/// Why it matters: the guard refuses the whole command line, so one protected path marked
/// among nine ordinary ones destroys none of them, and the operator learns that only after
/// browsing, marking and confirming.
#[test]
fn a_protected_path_cannot_be_marked() {
    // Given:
    let mut selector = open(Path::new("/"));
    select(&mut selector, "usr");

    // When:
    press(&mut selector, KeyCode::Char(' '));

    // Then:
    let text = screen_text(&mut selector);
    assert!(text.contains("! /usr"), "{text}");
    press(&mut selector, KeyCode::Enter);
    assert_eq!(selector.targets(), Vec::<PathBuf>::new());
}

/// A protected row reads as refused before anything is pressed.
///
/// Given: a selector at the root of the filesystem.
/// When: it is drawn.
/// Then: the `usr` row carries a muted `-`.
///
/// Why it matters: an operator reads the list before pressing anything, and a refusal that only
/// appears after a key makes them find each protected path by trying it.
#[test]
fn a_protected_row_reads_as_refused_before_it_is_pressed() {
    // Given:
    let mut selector = open(Path::new("/"));
    select(&mut selector, "usr");

    // When:
    let screen = render(&mut selector, 80, 24);

    // Then:
    assert_eq!(state_cell(&screen, "usr/").0, '-');
}

/// The up key never climbs into a directory the run protects.
///
/// Given: a selector two levels down in a temporary tree.
/// When: the up key is pressed far more times than the tree is deep.
/// Then: it stops in a directory the guard accepts, and the status row says why it stopped.
///
/// Why it matters: refusing to open in a protected directory buys nothing if the up key walks
/// into one, and every temporary directory on macOS sits under `/var`.
#[test]
fn the_selector_never_climbs_into_a_protected_directory() {
    // Given:
    let root = tree(&["a/b/file.txt"]);
    let mut selector = open(&root.path().join("a").join("b"));

    // When:
    for _ in 0..30 {
        press(&mut selector, KeyCode::Char('h'));
    }

    // Then:
    assert!(Guard::for_host().check(selector.directory()).is_ok());
    let text = screen_text(&mut selector);
    assert!(text.contains("! "), "{text}");
}

/// An ordinary path is marked, and a second press unmarks it.
///
/// Given: a temporary directory holding one file.
/// When: space is pressed on it once, and then again.
/// Then: the first press marks it, and the second leaves nothing marked.
///
/// Why it matters: a refusal rule that catches everything is a selector that does not work, and
/// a mark that only adds makes a mistaken mark unrecoverable without quitting.
#[test]
fn space_marks_an_ordinary_entry_and_a_second_press_unmarks_it() {
    // Given:
    let root = tree(&["notes.txt"]);
    let mut selector = open(root.path());

    // When:
    press(&mut selector, KeyCode::Char(' '));
    let once = render(&mut selector, 80, 24);
    press(&mut selector, KeyCode::Char(' '));
    let twice = render(&mut selector, 80, 24);

    // Then:
    assert_eq!(state_cell(&once, "notes.txt"), ('*', LOVE));
    assert_eq!(state_cell(&twice, "notes.txt").0, ' ');
}

/// A directory loads off the screen's thread, and quit answers while it loads.
///
/// Given: a selector on a tree with a subdirectory under the cursor.
/// When: the descend key is pressed, and q is pressed before the load finishes.
/// Then: the descend key asks for a load and the screen says loading, and q quits.
///
/// Why it matters: a slow or remote directory read on the screen's thread freezes the screen
/// with no way to leave it.
#[test]
fn a_directory_loads_off_the_screen_and_quit_answers_while_it_does() {
    // Given:
    let root = tree(&["sub/inner.txt"]);
    let mut selector = open(root.path());

    // When:
    let descend = selector.key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    let loading = screen_text(&mut selector);
    let quit = selector.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

    // Then:
    assert!(matches!(descend, Pick::Load(_)));
    assert!(loading.contains("loading"), "{loading}");
    assert!(matches!(quit, Pick::Quit));
}

/// The keys that change directory wait while a directory loads.
///
/// Given: a selector that asked to go into a directory.
/// When: the up key is pressed before the load finishes, and then the load finishes.
/// Then: the up key asks for nothing, and the selector is in the directory it asked for.
///
/// Why it matters: a second load in flight can finish first, and the late answer then shows
/// a directory the operator already left, where the next mark lands.
#[test]
fn the_keys_that_change_directory_wait_while_a_directory_loads() {
    // Given:
    let root = tree(&["sub/inner.txt"]);
    let mut selector = open(&root.path().canonicalize().unwrap());
    let Pick::Load(down) = selector.key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE))
    else {
        panic!("the descend key asks for a load");
    };

    // When:
    let up = selector.key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    selector.loaded(down.run());

    // Then:
    assert!(matches!(up, Pick::None));
    assert_eq!(
        selector.directory(),
        root.path().canonicalize().unwrap().join("sub")
    );
}

/// A mark holds when the operator leaves its directory and comes back.
///
/// Given: a file marked in a subdirectory.
/// When: the operator goes up and back down.
/// Then: the file still carries its mark, and the done key hands it over.
///
/// Why it matters: targets in several directories go to one run, and a mark lost on the way
/// up makes the whole selector single-select by accident.
#[test]
fn a_mark_survives_leaving_the_directory_and_coming_back() {
    // Given:
    let root = tree(&["sub/inner.txt"]);
    let root_path = root.path().canonicalize().unwrap();
    let mut selector = open(&root_path);
    press(&mut selector, KeyCode::Char('l'));
    press(&mut selector, KeyCode::Char(' '));

    // When:
    press(&mut selector, KeyCode::Char('h'));
    press(&mut selector, KeyCode::Char('l'));

    // Then:
    let screen = render(&mut selector, 80, 24);
    assert_eq!(state_cell(&screen, "inner.txt").0, '*');
    assert!(matches!(press(&mut selector, KeyCode::Enter), Pick::Done));
    assert_eq!(
        selector.targets(),
        [root_path.join("sub").join("inner.txt")]
    );
}

/// Moving to another directory puts the cursor on its first entry and drops the filter.
///
/// Given: a filter that shows two entries, with the cursor on the second, a directory.
/// When: the operator goes into that directory.
/// Then: the new listing shows every entry, and the cursor is on the first.
///
/// Why it matters: a cursor past the end of a shorter listing selects nothing, so space and the
/// descend key stop working with no sign why, and a filter typed for one directory hides
/// entries in the next.
#[test]
fn moving_directory_resets_the_cursor_and_the_filter() {
    // Given:
    let root = tree(&["a.txt", "b.txt", "zeta/one.txt", "zeta/two.txt"]);
    let mut selector = open(root.path());
    select(&mut selector, "a");
    press(&mut selector, KeyCode::Down);

    // When:
    press(&mut selector, KeyCode::Char('l'));

    // Then:
    let text = screen_text(&mut selector);
    assert!(text.contains("1/2  0 marked"), "{text}");
}

/// While the filter is being typed, every key but the ones that leave it is text.
///
/// Given: a selector with the filter box open.
/// When: q, space, l and h are typed.
/// Then: none of them quits, marks or moves, and the box holds all four.
///
/// Why it matters: a filename holds spaces and every letter the selector binds, and a q that
/// quits halfway through a name throws away the marks the operator was building.
#[test]
fn the_filter_owns_its_own_keys() {
    // Given:
    let root = tree(&["q l h.txt"]);
    let mut selector = open(root.path());
    press(&mut selector, KeyCode::Char('/'));

    // When:
    let picks: Vec<Pick> = "q lh"
        .chars()
        .map(|character| press(&mut selector, KeyCode::Char(character)))
        .collect();

    // Then:
    assert!(picks.iter().all(|pick| matches!(pick, Pick::None)));
    let text = screen_text(&mut selector);
    assert!(text.contains("filter: q lh"), "{text}");
    press(&mut selector, KeyCode::Enter);
    assert!(screen_text(&mut selector).contains("0 marked"));
}

/// Escape clears the filter before it quits, and ctrl+c quits from inside the filter.
///
/// Given: a selector with a filter applied, and another with the box open.
/// When: escape is pressed twice on the first, and ctrl+c on the second.
/// Then: the first escape only clears the filter, the second quits, and ctrl+c quits.
///
/// Why it matters: escape is pressed to back out of a filter, and a screen that routes ctrl+c
/// into a text input cannot be left at all.
#[test]
fn escape_clears_the_filter_first_and_ctrl_c_always_quits() {
    // Given:
    let root = tree(&["a.txt", "b.txt"]);
    let mut filtered = open(root.path());
    select(&mut filtered, "a");
    let mut typing = open(root.path());
    press(&mut typing, KeyCode::Char('/'));

    // When:
    let first = press(&mut filtered, KeyCode::Esc);
    let second = press(&mut filtered, KeyCode::Esc);
    let interrupt = typing.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));

    // Then:
    assert!(matches!(first, Pick::None));
    assert!(matches!(second, Pick::Quit));
    assert!(matches!(interrupt, Pick::Quit));
}

/// Quitting hands over nothing, whatever was marked.
///
/// Given: a selector with a file marked.
/// When: the operator quits.
/// Then: the selector hands over no targets.
///
/// Why it matters: a quit is how an operator leaves a screen opened by mistake, and this one
/// feeds a run that cannot be undone.
#[test]
fn quitting_hands_over_nothing() {
    // Given:
    let root = tree(&["notes.txt"]);
    let mut selector = open(root.path());
    press(&mut selector, KeyCode::Char(' '));

    // When:
    let pick = press(&mut selector, KeyCode::Char('q'));

    // Then:
    assert!(matches!(pick, Pick::Quit));
    assert_eq!(selector.targets(), Vec::<PathBuf>::new());
}

/// A symlink to a directory is never entered, and every entry says what kind it is.
///
/// Given: a tree holding a directory, a file, and a symlink to a directory elsewhere.
/// When: it is drawn, and the descend key is pressed on the symlink.
/// Then: the names read `dir/`, `file.txt` and `link@`, and the selector stays where it was.
///
/// Why it matters: marking a directory destroys the whole tree under it, and marking a symlink
/// destroys the link and never what it points at, so neither may read like a plain file, and
/// following a link would show the operator a directory outside the one they are browsing.
#[test]
fn a_symlink_is_never_entered_and_each_entry_says_what_it_is() {
    // Given:
    let root = tree(&["dir/inner.txt", "file.txt"]);
    let elsewhere = tree(&["outside.txt"]);
    symlink(elsewhere.path(), root.path().join("link")).unwrap();
    let mut selector = open(root.path());
    let before = selector.directory().to_path_buf();

    // When:
    let text = screen_text(&mut selector);
    select(&mut selector, "link");
    press(&mut selector, KeyCode::Char('l'));

    // Then:
    for label in ["dir/", "file.txt", "link@"] {
        assert!(text.contains(label), "{label}:\n{text}");
    }
    assert_eq!(selector.directory(), before);
}

/// The cursor row carries a gold bracket pair around an iris name.
///
/// Given: a listing of two files, with the cursor moved to the second.
/// When: it is drawn.
/// Then: the second row is bracketed in gold with an iris name, and the first is not bracketed.
///
/// Why it matters: space marks the row under the cursor, and a pair on the wrong row marks a
/// file the operator was not looking at.
#[test]
fn the_cursor_row_carries_the_bracket_pair() {
    // Given:
    let root = tree(&["first.txt", "second.txt"]);
    let mut selector = open(root.path());
    press(&mut selector, KeyCode::Char('j'));

    // When:
    let screen = render(&mut selector, 80, 24);

    // Then:
    let (y, line) = row_of(&screen, "second.txt");
    let open = line.iter().position(|&cell| cell == '[').unwrap() as u16;
    let name = line.iter().position(|&cell| cell == 's').unwrap() as u16;
    assert_eq!(screen[(open, y)].fg, GOLD);
    assert_eq!(screen[(name, y)].fg, IRIS);
    assert!(!row_of(&screen, "first.txt").1.contains(&'['));
}

/// A row fits its column at every width, wide characters included.
///
/// Given: a file whose name is long and made of wide characters.
/// When: the selector is drawn at every width from 12 to 90 columns.
/// Then: the frame's right rule is in the same column on every row of the listing.
///
/// Why it matters: a wide character is one character and two cells, and a row wider than its
/// column tears the frame around it.
#[test]
fn a_row_fits_its_column_at_every_width() {
    // Given:
    let root = tree(&["資料庫備份檔案的長名稱與更多的文字在這裡.txt", "plain.txt"]);
    let mut selector = open(root.path());

    for width in 12..=90 {
        // When:
        let screen = render(&mut selector, width, 24);

        // Then:
        let (top, top_line) = row_of(&screen, "┌");
        let (bottom, _) = row_of(&screen, "└");
        let right = top_line.len() - 1;
        for row in top + 1..bottom {
            let line: Vec<char> = lines(&screen)[usize::from(row)].chars().collect();
            assert_eq!(
                line.get(right),
                Some(&'│'),
                "{width}: {}",
                line.iter().collect::<String>()
            );
        }
    }
}

/// The selector says where it is, how far down the listing it is, and how much is marked.
///
/// Given: a directory of three files, with the second marked and the cursor on it.
/// When: it is drawn.
/// Then: the frame title names the directory, and the status row reads 2/3 and 1 marked.
///
/// Why it matters: the frame title is the only thing that says which directory a mark lands
/// in, and the count is what tells a set of three from a set of four.
#[test]
fn the_selector_says_where_it_is_and_what_is_marked() {
    // Given:
    let root = tree(&["a.txt", "b.txt", "c.txt"]);
    let mut selector = open(root.path());
    press(&mut selector, KeyCode::Down);
    press(&mut selector, KeyCode::Char(' '));

    // When:
    let text = screen_text(&mut selector);

    // Then:
    let name = root.path().file_name().unwrap().to_str().unwrap();
    assert!(text.contains(name), "{text}");
    assert!(text.contains("2/3  1 marked"), "{text}");
}

/// An applied filter says how much of the directory it hides.
///
/// Given: a directory of three files.
/// When: a filter that matches one of them is applied.
/// Then: the status row reads 1/1 of 3.
///
/// Why it matters: a filtered listing looks exactly like a small directory, so nothing else
/// says that the entries it hid are still there.
#[test]
fn an_applied_filter_says_what_it_is_hiding() {
    // Given:
    let root = tree(&["alpha.txt", "beta.txt", "gamma.txt"]);
    let mut selector = open(root.path());

    // When:
    select(&mut selector, "BETA");

    // Then:
    let text = screen_text(&mut selector);
    assert!(text.contains("1/1 of 3  0 marked"), "{text}");
}

/// A directory nobody can read says so, rather than reading as empty.
///
/// Given: a subdirectory with no permissions.
/// When: the operator goes into it.
/// Then: the status row carries a `!` and the reason.
///
/// Why it matters: an unreadable directory looks exactly like an empty one, and an operator
/// who believes it empty is looking at nothing while its files are still there.
#[test]
fn the_selector_reports_a_directory_it_cannot_read() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    // Given:
    let root = tree(&["locked/inner.txt"]);
    let locked = root.path().join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let mut selector = open(root.path());

    // When:
    press(&mut selector, KeyCode::Char('l'));
    let text = screen_text(&mut selector);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();

    // Then:
    assert!(text.contains("! "), "{text}");
    assert!(text.to_lowercase().contains("permission denied"), "{text}");
}

/// A selector opened on a relative path can still go up.
///
/// Given: a selector opened on the relative path `src`.
/// When: the up key is pressed.
/// Then: the selector is in the directory that holds `src`.
///
/// Why it matters: the parent of a relative name with one part is the empty path, so a selector
/// that keeps the name it was given can never leave where it started.
#[test]
fn a_selector_opened_on_a_relative_path_can_go_up() {
    // Given:
    let mut selector = open(Path::new("src"));

    // When:
    press(&mut selector, KeyCode::Char('h'));

    // Then:
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();
    assert_eq!(selector.directory(), crate_root);
}

/// A name cannot drive the terminal it is listed on.
///
/// Given: a file whose name holds an escape sequence.
/// When: the selector lists it.
/// Then: the screen shows the escape as `\x1b`, and holds no escape character.
///
/// Why it matters: a file name is chosen by whoever wrote the file, and a control character in
/// it can retitle the window, move the cursor, or hide the rest of the listing.
#[test]
fn a_name_cannot_drive_the_terminal_it_is_listed_on() {
    // Given:
    let root = tree(&["red\x1b[31mtext.txt"]);
    let mut selector = open(root.path());

    // When:
    let text = screen_text(&mut selector);

    // Then:
    assert!(text.contains(r"red\x1b[31mtext.txt"), "{text}");
    assert!(!text.contains('\x1b'));
}

/// A selector over a small project, with one file marked and the cursor on a directory.
fn project() -> (tempfile::TempDir, Selector) {
    let root = tree(&[
        "project/archive/old.txt",
        "project/keys/id_ed25519",
        "project/notes.txt",
        "project/report-final-v2.pdf",
    ]);
    let mut selector = open(&root.path().join("project"));
    select(&mut selector, "notes");
    press(&mut selector, KeyCode::Char(' '));
    press(&mut selector, KeyCode::Esc);
    press(&mut selector, KeyCode::Down);
    (root, selector)
}

/// Compares a render with a reviewed snapshot. The frame title holds the path of a temporary
/// directory, which differs on every machine, so its row is checked for the project name alone.
fn assert_snapshot(screen: &Buffer, expected: &[&str]) {
    let mut shown = lines(screen);
    while shown.last().is_some_and(String::is_empty) {
        shown.pop();
    }
    assert_eq!(shown.len(), expected.len(), "{}", shown.join("\n"));
    for (line, expected) in shown.iter().zip(expected) {
        if *expected == "TITLE" {
            assert!(line.contains("/project ─"), "{line}");
        } else {
            assert_eq!(line, expected);
        }
    }
}

/// A selector at a phone width matches the reviewed snapshot.
///
/// Given: a small project with one file marked and the cursor on a directory.
/// When: it is drawn on a terminal of 40 by 20.
/// Then: every row matches the snapshot reviewed when the layout was set.
///
/// Why it matters: the layout is cell exact, and a change of one cell in a rule or a gap is
/// invisible in a review of the code and obvious on the screen.
#[test]
fn a_selector_matches_its_snapshot_at_a_phone_width() {
    // Given:
    let (_root, mut selector) = project();

    // When:
    let screen = render(&mut selector, 40, 20);

    // Then:
    let expected = [
        "",
        "  blank",
        "",
        "  2/4  1 marked               [q] quit",
        "  marked: 1 file, 7 bytes",
        "",
        "TITLE",
        "  │     archive/                     │",
        "  │ [   keys/                     ]  │",
        "  │   * notes.txt                    │",
        "  │     report-final-v2.pdf          │",
        "  │                                  │",
        "  └──────────────────────────────────┘",
        "  ┌─ preview ────────────────────────┐",
        "  │ marking it destroys              │",
        "  │ 1 file                           │",
        "  │ 1 directory                      │",
        "  │ 7 bytes                          │",
        "  │                                  │",
        "  └──────────────────────────────────┘",
    ];
    assert_snapshot(&screen, &expected);
}

/// A selector at a desktop width matches the reviewed snapshot.
///
/// Given: a small project with one file marked and the cursor on a directory.
/// When: it is drawn on a terminal of 100 by 30.
/// Then: every row matches the snapshot reviewed when the layout was set.
///
/// Why it matters: the layout is cell exact, and a change of one cell in a rule or a gap is
/// invisible in a review of the code and obvious on the screen.
#[test]
fn a_selector_matches_its_snapshot_at_a_desktop_width() {
    // Given:
    let (_root, mut selector) = project();

    // When:
    let screen = render(&mut selector, 100, 30);

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
        "  2/4  1 marked                               [space] mark  [enter] done  [q] quit",
        "  marked: 1 file, 7 bytes",
        "",
        "TITLE",
        "  │     archive/                         │ │ marking it destroys                 │",
        "  │ [   keys/                         ]  │ │ 1 file                              │",
        "  │   * notes.txt                        │ │ 1 directory                         │",
        "  │     report-final-v2.pdf              │ │ 7 bytes                             │",
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
        "  │                                      │ │                                     │",
        "  │                                      │ │                                     │",
        "  │                                      │ │                                     │",
        "  └──────────────────────────────────────┘ └─────────────────────────────────────┘",
    ];
    assert_snapshot(&screen, &expected);
}

/// The preview pane shows the head of the file under the cursor, and follows the cursor.
///
/// Given: a directory of two text files.
/// When: the selector opens, and the cursor moves to the second file.
/// Then: the pane first shows the lines of the first file, and then those of the second only.
///
/// Why it matters: the head of a file is what tells the operator which file a mark lands on,
/// and a pane that lags the cursor shows them the wrong one.
#[test]
fn the_preview_shows_the_file_under_the_cursor() {
    // Given:
    let root = tree(&[]);
    fs::write(root.path().join("a.txt"), "alpha contents\n").unwrap();
    fs::write(root.path().join("b.txt"), "bravo contents\n").unwrap();

    // When:
    let mut selector = open(root.path());
    let first = screen_text(&mut selector);
    press(&mut selector, KeyCode::Down);
    let second = screen_text(&mut selector);

    // Then:
    assert!(first.contains("alpha contents"), "{first}");
    assert!(
        second.contains("bravo contents") && !second.contains("alpha contents"),
        "{second}"
    );
}

/// A binary file previews as a hex dump with its printable bytes beside it.
///
/// Given: a file that starts with an executable header.
/// When: the selector shows it.
/// Then: the pane holds the header bytes in hex and the printable text `.ELF`.
///
/// Why it matters: binary contents drawn as text scramble the pane, and the bytes are what
/// identify the file.
#[test]
fn a_binary_file_previews_as_a_hex_dump() {
    // Given:
    let root = tree(&[]);
    let mut contents = vec![0x7f, b'E', b'L', b'F'];
    contents.extend([0u8; 60]);
    fs::write(root.path().join("program"), contents).unwrap();

    // When:
    let mut selector = open(root.path());

    // Then:
    let text = screen_text(&mut selector);
    assert!(text.contains("7f 45 4c 46"), "{text}");
    assert!(text.contains(".ELF"), "{text}");
}

/// The marked set is counted off the selector, and the header says so until the count arrives.
///
/// Given: a directory holding two files of 10 and 20 bytes.
/// When: the operator marks it, before and after the count finishes.
/// Then: the header first says it is counting, and then reads 2 files, 1 directory and 30 bytes.
///
/// Why it matters: marking a large directory means walking it, and a selector that walked it on
/// its own thread would freeze until the walk ended, with no way to leave.
#[test]
fn the_marked_set_is_counted_off_the_selector() {
    // Given:
    let root = tree(&["project/a.txt", "project/sub/b.txt"]);
    fs::write(root.path().join("project").join("a.txt"), vec![b'x'; 10]).unwrap();
    fs::write(
        root.path().join("project").join("sub").join("b.txt"),
        vec![b'x'; 20],
    )
    .unwrap();
    let mut selector = open(root.path());

    // When:
    selector.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let work = selector.work();
    let pending = screen_text(&mut selector);
    for work in work {
        selector.finished(work.run());
    }
    let counted = screen_text(&mut selector);

    // Then:
    assert!(pending.contains("counting the marked set"), "{pending}");
    assert!(
        counted.contains("marked: 2 files, 2 directories, 30 bytes"),
        "{counted}"
    );
}

/// A total that a newer mark replaced is stopped and never shown.
///
/// Given: a total asked for after one mark, and another after a second mark.
/// When: the newer total finishes, and then the older one runs.
/// Then: the older one stops without a result, and the header shows the total of both marks.
///
/// Why it matters: an older total that lands last shows a set the operator already changed,
/// and a walk nobody will read holds a thread for the size of the tree.
#[test]
fn a_total_that_a_newer_mark_replaced_is_never_shown() {
    // Given:
    let root = tree(&["a.txt", "b.txt"]);
    let mut selector = open(root.path());
    selector.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let older = selector.work();
    selector.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    selector.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let newer = selector.work();

    // When:
    for work in newer {
        selector.finished(work.run());
    }
    let late: Vec<_> = older.iter().map(|work| work.run()).collect();
    let stopped = late.iter().any(|finished| {
        matches!(
            finished,
            blank::selector::Finished::Totals { totals: None, .. }
        )
    });
    for finished in late {
        selector.finished(finished);
    }

    // Then:
    assert!(stopped, "the older total ran to its end");
    let text = screen_text(&mut selector);
    assert!(text.contains("marked: 2 files, 14 bytes"), "{text}");
}

/// Closing the selector stops the walks it started.
///
/// Given: a marked directory whose total was asked for and has not run.
/// When: the selector is closed.
/// Then: the total stops without a result.
///
/// Why it matters: the run that follows needs the disk, and a walk for a pane nobody sees
/// anymore competes with it for as long as the tree is large.
#[test]
fn closing_the_selector_stops_the_walks_it_started() {
    // Given:
    let root = tree(&["project/a.txt"]);
    let mut selector = open(root.path());
    selector.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let work = selector.work();

    // When:
    drop(selector);

    // Then:
    for work in work {
        if let finished @ blank::selector::Finished::Totals { .. } = work.run() {
            assert!(matches!(
                finished,
                blank::selector::Finished::Totals { totals: None, .. }
            ));
        }
    }
}

/// A total that finished before a newer mark, and arrives after it, is ignored.
///
/// Given: a total that ran to its end after one mark, and a second mark made before its result
/// arrived.
/// When: the older result arrives.
/// Then: the header still says it is counting.
///
/// Why it matters: a result already on its way cannot be stopped, and taking it would show the
/// totals of a set the operator already changed.
#[test]
fn a_total_that_arrives_after_a_newer_mark_is_ignored() {
    // Given:
    let root = tree(&["a.txt", "b.txt"]);
    let mut selector = open(root.path());
    selector.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let arrived: Vec<_> = selector.work().iter().map(|work| work.run()).collect();
    selector.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    selector.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let _newer = selector.work();

    // When:
    for finished in arrived {
        selector.finished(finished);
    }

    // Then:
    let text = screen_text(&mut selector);
    assert!(text.contains("counting the marked set"), "{text}");
}

/// The totals row keeps whole numbers on a narrow screen.
///
/// Given: a marked directory of 300000 bytes in two files.
/// When: the selector is drawn at every width from 12 to 90 columns.
/// Then: wherever a size shows it is whole, and from 20 columns up the size always shows.
///
/// Why it matters: a size cut to `293.` reads as 293 bytes, three orders of magnitude short of
/// what the mark destroys.
#[test]
fn the_totals_row_keeps_whole_numbers_on_a_narrow_screen() {
    // Given:
    let root = tree(&["project/sub/a.txt"]);
    fs::write(
        root.path().join("project").join("big.bin"),
        vec![0; 300_000],
    )
    .unwrap();
    let mut selector = open(root.path());
    press(&mut selector, KeyCode::Char(' '));

    for width in 12..=90 {
        // When:
        let screen = render(&mut selector, width, 30);

        // Then:
        let row = lines(&screen).into_iter().find(|line| line.contains("293"));
        if let Some(row) = &row {
            assert!(row.contains("293.0 KiB"), "{width}: {row}");
        }
        assert!(
            width < 20 || row.is_some(),
            "{width}: no size on the screen"
        );
    }
}

/// A taller screen shows again the entries a short one scrolled away.
///
/// Given: a selector drawn on a short terminal with the cursor on the last of eight entries.
/// When: it is drawn on a tall one.
/// Then: the first entry is on the screen again.
///
/// Why it matters: a terminal is resized while the screen is up, and a list that keeps its top
/// entries hidden above empty space reads as a shorter directory.
#[test]
fn a_taller_screen_shows_again_the_entries_a_short_one_scrolled_away() {
    // Given:
    let names: Vec<String> = (1..=8).map(|index| format!("file-{index}.txt")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let root = tree(&refs);
    let mut selector = open(root.path());
    press(&mut selector, KeyCode::End);
    render(&mut selector, 40, 14);

    // When:
    let text = lines(&render(&mut selector, 100, 40)).join("\n");

    // Then:
    assert!(text.contains("file-1.txt"), "{text}");
}
