use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui_image::picker::Picker;
use ratatui_image::picker::cap_parser::QueryStdioOptions;

use crate::engine::{Options, Stage, Update, destroy};
use crate::guard::Guard;
use crate::plan::{Incomplete, Plan};
use crate::review::{Answer, Review};
use crate::selector::{Finished, LoadRequest, Loaded, Pick, Selector, Work};
use crate::view::{Action, Outcome, RunView};

/// How long the screen waits for a key before it draws again. It sets the pace of the busy
/// mark, and every report that arrives in between lands in the next frame.
const FRAME: Duration = Duration::from_millis(80);
/// A local terminal answers the graphics query in well under this, and one that never answers
/// would otherwise hold the first screen closed for the two seconds the crate waits by default.
const GRAPHICS_QUERY_TIMEOUT: Duration = Duration::from_millis(500);

/// How this terminal draws images, asked once a session, because the query reads standard input
/// and has to finish before the first key is read.
static PICKER: OnceLock<Picker> = OnceLock::new();

pub struct Ended {
    pub report: Result<(), Incomplete>,
    pub outcome: Outcome,
    /// Set when the terminal failed during the run. The run was then stopped, and the report
    /// still names every target it did not destroy.
    pub interface: io::Result<()>,
}

enum Feed {
    Target {
        verb: &'static str,
        path: PathBuf,
        stage: Stage,
    },
    Progress {
        done: u64,
        total: u64,
    },
    Finished(Result<(), Incomplete>),
}

/// Raw mode and the alternate screen, put back when this is dropped, a panic included, so a
/// failed run never leaves the operator's terminal unusable.
struct FullScreen(Terminal<CrosstermBackend<Stdout>>);

impl FullScreen {
    fn enter() -> io::Result<FullScreen> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(FullScreen(terminal)),
            Err(error) => {
                let _ = execute!(io::stdout(), LeaveAlternateScreen);
                let _ = disable_raw_mode();
                Err(error)
            }
        }
    }
}

impl Drop for FullScreen {
    fn drop(&mut self) {
        let _ = execute!(self.0.backend_mut(), LeaveAlternateScreen);
        let _ = self.0.show_cursor();
        let _ = disable_raw_mode();
    }
}

/// The session's picker, asked of the terminal on the first call. Call it inside a full screen
/// and before any event is read.
fn picker() -> Picker {
    PICKER
        .get_or_init(|| {
            let options = QueryStdioOptions {
                timeout: GRAPHICS_QUERY_TIMEOUT,
                ..QueryStdioOptions::default()
            };
            Picker::from_query_stdio_with_options(options).unwrap_or_else(|_| Picker::halfblocks())
        })
        .clone()
}

/// Runs the plan on the full screen. `record` runs on the engine thread for every update, before
/// the screen sees it, so a failed screen cannot cost the manifest a line. The screen stays up
/// after the run settles until the operator closes it, and a stop closes it once the engine has
/// reported, so the verdict is never drawn ahead of the report.
///
/// An error means the screen could not open, and nothing was destroyed.
pub fn show(
    plan: &Plan,
    options: Options,
    stop: &AtomicBool,
    record: &mut (dyn FnMut(&Update<'_>) + Send),
) -> io::Result<Ended> {
    let mut screen = FullScreen::enter()?;
    let mut view = RunView::new();
    let (sender, receiver) = mpsc::channel();
    let (report, interface) = thread::scope(|scope| {
        scope.spawn(move || {
            let report = destroy(plan, options, stop, &mut |update| {
                record(&update);
                // The screen is gone only after a panic there, and nothing would then show
                // the run what it destroys.
                let sent = sender.send(match update {
                    Update::Target { verb, path, stage } => Feed::Target {
                        verb,
                        path: path.to_path_buf(),
                        stage,
                    },
                    Update::Progress { done, total } => Feed::Progress { done, total },
                });
                if sent.is_err() {
                    stop.store(true, Ordering::Relaxed);
                }
            });
            let _ = sender.send(Feed::Finished(report));
        });
        let mut report = None;
        let mut closing = false;
        let interface = loop {
            loop {
                match receiver.try_recv() {
                    Ok(Feed::Target { verb, path, stage }) => view.target(verb, &path, stage),
                    Ok(Feed::Progress { done, total }) => view.progress(done, total),
                    Ok(Feed::Finished(finished)) => {
                        // An interrupt from outside the terminal stops the run the way q does.
                        // The engine read the flag before it reported, and the channel orders
                        // that read before this one, so a stop that ended the run is seen here.
                        if stop.load(Ordering::Relaxed) {
                            view.cancel();
                            closing = true;
                        }
                        view.finish(&finished);
                        report = Some(finished);
                    }
                    Err(TryRecvError::Empty) => break,
                    // The scope then raises the engine's own panic.
                    Err(TryRecvError::Disconnected) if report.is_none() => {
                        panic!("the engine stopped without a report")
                    }
                    Err(TryRecvError::Disconnected) => break,
                }
            }
            if closing && report.is_some() {
                break Ok(());
            }
            if let Err(error) = screen.0.draw(|frame| view.draw(frame)) {
                break Err(error);
            }
            match next_key(&mut view) {
                Ok(Action::Stop) => {
                    stop.store(true, Ordering::Relaxed);
                    closing = true;
                }
                Ok(Action::None) => {}
                Err(error) => break Err(error),
            }
        };
        if interface.is_err() {
            stop.store(true, Ordering::Relaxed);
        }
        let report = report.unwrap_or_else(|| {
            receiver
                .iter()
                .find_map(|feed| match feed {
                    Feed::Finished(finished) => Some(finished),
                    _ => None,
                })
                .expect("the engine thread reports before it returns")
        });
        (report, interface)
    });
    drop(screen);
    view.finish(&report);
    // A failed screen stopped the run, so a run it left short is a cancelled one.
    let outcome = match (&interface, &report) {
        (Err(_), Err(_)) => Outcome::Cancelled,
        _ => view.outcome(),
    };
    Ok(Ended {
        report,
        outcome,
        interface,
    })
}

fn next_key(view: &mut RunView) -> io::Result<Action> {
    if !event::poll(FRAME)? {
        view.tick();
        return Ok(Action::None);
    }
    match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => Ok(view.key(key)),
        _ => Ok(Action::None),
    }
}

/// Opens the selector on `start` and returns the paths the operator marked, in a stable order.
/// It returns nothing when the operator quit, which is the same answer as marking nothing.
/// Each directory is read on a thread of its own, so a slow one shows a loading state and every
/// key still answers.
pub fn pick(start: &Path, guard: Arc<Guard>, options: Options) -> io::Result<Vec<PathBuf>> {
    let mut screen = FullScreen::enter()?;
    let (mut selector, first) = Selector::open(start, guard, options, picker());
    let (sender, receiver) = mpsc::channel();
    let read = {
        let sender = sender.clone();
        move |load: LoadRequest| {
            let sender = sender.clone();
            thread::spawn(move || {
                let _ = sender.send(Read::Loaded(load.run()));
            });
        }
    };
    // A newer request stops the walk of the one before it, and closing the selector stops
    // both, so a walk nobody will read ends at its next entry.
    let run = move |work: Work| {
        let sender = sender.clone();
        thread::spawn(move || {
            let _ = sender.send(Read::Finished(work.run()));
        });
    };
    read(first);
    loop {
        for done in receiver.try_iter() {
            match done {
                Read::Loaded(loaded) => selector.loaded(loaded),
                Read::Finished(finished) => selector.finished(finished),
            }
        }
        for work in selector.work() {
            run(work);
        }
        screen.0.draw(|frame| selector.draw(frame))?;
        if !event::poll(FRAME)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match selector.key(key) {
            Pick::None => {}
            Pick::Load(load) => read(load),
            Pick::Done => return Ok(selector.targets()),
            Pick::Quit => return Ok(Vec::new()),
        }
    }
}

enum Read {
    Loaded(Loaded),
    Finished(Finished),
}

/// Shows the review of `plan` and returns whether the operator typed yes and pressed enter.
/// Every other way out of the screen destroys nothing.
pub fn review(plan: &Plan, guard: Arc<Guard>, options: &Options) -> io::Result<bool> {
    let mut screen = FullScreen::enter()?;
    let mut review = Review::new(plan, guard, options, picker());
    let (sender, receiver) = mpsc::channel();
    loop {
        for result in receiver.try_iter() {
            review.finished(result);
        }
        if let Some(request) = review.work() {
            let sender = sender.clone();
            thread::spawn(move || {
                let _ = sender.send(request.run());
            });
        }
        screen.0.draw(|frame| review.draw(frame))?;
        if !event::poll(FRAME)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match review.key(key) {
            Answer::Waiting => {}
            Answer::Destroy => return Ok(true),
            Answer::Quit => return Ok(false),
        }
    }
}
