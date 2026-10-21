use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use blank::guard::Guard;
use blank::preview::{Preview, preview};
use blank::render::{Kind, Renderer, TOOL_TIMEOUT};
use image::{DynamicImage, GenericImageView, Rgba};
use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn look(path: &Path, renderer: &Renderer) -> Preview {
    preview(path, &Guard::for_host(), renderer, &AtomicBool::new(false))
        .expect("nothing stops the preview")
}

/// A tool directory that holds `tool`, which starts a process of its own, records both process
/// ids and the temporary directory it was given, then waits for longer than any test waits.
fn slow_tool(tool: &str) -> (tempfile::TempDir, PathBuf) {
    let tools = tempfile::tempdir().unwrap();
    let started = tools.path().join("started");
    let script = format!(
        "#!/bin/sh\n/bin/sleep 60 &\necho \"$$ $! $TMPDIR\" > '{started}.part'\n/bin/mv '{started}.part' '{started}'\nwait\n",
        started = started.display()
    );
    let path = tools.path().join(tool);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    (tools, started)
}

/// The processes and the temporary directory the slow tool recorded, once it started.
fn started(record: &Path) -> ([Pid; 2], PathBuf) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !record.exists() {
        assert!(Instant::now() < deadline, "the tool never started");
        thread::sleep(Duration::from_millis(10));
    }
    let text = fs::read_to_string(record).unwrap();
    let mut fields = text.trim_end().splitn(3, ' ');
    let mut pid = || Pid::from_raw(fields.next().unwrap().parse().unwrap());
    let pids = [pid(), pid()];
    (pids, PathBuf::from(fields.next().unwrap()))
}

/// A process started by the tool dies on its own schedule after the kill, and is a zombie until
/// the process that adopts it reaps it, so a look straight after the kill can still find it.
fn assert_gone(pids: [Pid; 2]) {
    let ended = |pid: Pid| {
        kill(pid, None) == Err(Errno::ESRCH)
            || state(pid).is_some_and(|state| state.starts_with('Z'))
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    while !pids.iter().all(|&pid| ended(pid)) {
        if Instant::now() >= deadline {
            let states: Vec<_> = pids.iter().map(|&pid| (pid, state(pid))).collect();
            panic!("the tool or a process it started still runs: {states:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn state(pid: Pid) -> Option<String> {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("ps runs");
    let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!state.is_empty()).then_some(state)
}

/// A PDF of one page of 200 by 100 points, filled red, with a correct cross-reference table.
fn pdf(path: &Path) {
    let content = "1 0 0 rg 0 0 200 100 re f\n";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>".to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
    ];
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let xref = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    fs::write(path, out).unwrap();
}

/// An SVG of 120 by 80, filled red, whose drawing starts past the head a preview reads.
fn svg(path: &Path) {
    let padding = "x".repeat(5000);
    let text = format!(
        "<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"120\" height=\"80\">\n<!-- {padding} -->\n<rect width=\"120\" height=\"80\" fill=\"red\"/>\n</svg>\n"
    );
    fs::write(path, text).unwrap();
}

/// Whether `tool` is on the search path of this test run. A test that needs it skips without it,
/// so the gate gives the same answer on a machine without the tool.
fn installed(tool: &str) -> bool {
    let found = env::var_os("PATH")
        .is_some_and(|search| env::split_paths(&search).any(|dir| dir.join(tool).is_file()));
    if !found {
        eprintln!("skipped: {tool} is not installed");
    }
    found
}

fn is_red(pixel: Rgba<u8>) -> bool {
    let [red, green, blue, _] = pixel.0;
    red > 200 && green < 60 && blue < 60
}

fn centre(image: &DynamicImage) -> Rgba<u8> {
    image.get_pixel(image.width() / 2, image.height() / 2)
}

/// The picture a tool drew of `path`, through the tools on the search path.
fn rendered(path: &Path, expected: Kind) -> DynamicImage {
    let shown = look(path, &Renderer::for_host());
    let Preview::Rendered { kind, image } = shown else {
        panic!("{path:?} previews as a picture, not {shown:?}");
    };
    assert_eq!(kind, expected);
    image
}

/// A PDF previews as its first page, drawn by pdftoppm.
///
/// Given: a PDF of one red page.
/// When: it is previewed on a machine with pdftoppm.
/// Then: the preview is a picture of a PDF, red at its centre.
///
/// Why it matters: a PDF is told apart by what its first page shows, and its head bytes are the
/// same for every PDF.
#[test]
fn a_pdf_previews_as_its_first_page() {
    if !installed("pdftoppm") {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("report.pdf");
    pdf(&path);

    // When:
    let image = rendered(&path, Kind::Pdf);

    // Then:
    assert!(is_red(centre(&image)), "{:?}", centre(&image));
}

/// A video previews as a frame, drawn by ffmpegthumbnailer.
///
/// Given: a red video of one second, `tests/fixtures/red.mp4`, made with
/// `ffmpeg -f lavfi -i color=c=red:size=64x48:duration=1:rate=5 -pix_fmt yuv420p -c:v libx264`.
/// When: it is previewed on a machine with ffmpegthumbnailer.
/// Then: the preview is a picture of a video, red at its centre.
///
/// Why it matters: a video is told apart by what it shows, and its container head says nothing
/// about which recording it is.
#[test]
fn a_video_previews_as_a_frame() {
    if !installed("ffmpegthumbnailer") {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("clip.mp4");
    fs::copy(Path::new(FIXTURES).join("red.mp4"), &path).unwrap();

    // When:
    let image = rendered(&path, Kind::Video);

    // Then:
    assert!(is_red(centre(&image)), "{:?}", centre(&image));
}

/// An SVG previews as its drawing, read from its first byte.
///
/// Given: a red SVG whose drawing starts after a comment longer than the head a preview reads.
/// When: it is previewed on a machine with magick.
/// Then: the preview is a picture of an SVG, red at its centre.
///
/// Why it matters: the tool reads the file the preview already opened, past its head, so a tool
/// that read on from where the head stopped would draw nothing or fail.
#[test]
fn an_svg_previews_as_its_drawing() {
    if !installed("magick") {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("drawing.svg");
    svg(&path);

    // When:
    let image = rendered(&path, Kind::Svg);

    // Then:
    assert!(is_red(centre(&image)), "{:?}", centre(&image));
}

/// A HEIC photo previews as its picture, drawn by magick.
///
/// Given: a red HEIC of 64 by 48, `tests/fixtures/red.heic`, made with
/// `magick -size 64x48 xc:red -quality 50`.
/// When: it is previewed on a machine with magick.
/// Then: the preview is a picture of a HEIC, red at its centre.
///
/// Why it matters: a phone saves its photos as HEIC, and the image decoders inside blank do not
/// read it.
#[test]
fn a_heic_photo_previews_as_its_picture() {
    if !installed("magick") {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("photo.heic");
    fs::copy(Path::new(FIXTURES).join("red.heic"), &path).unwrap();

    // When:
    let image = rendered(&path, Kind::Heic);

    // Then:
    assert!(is_red(centre(&image)), "{:?}", centre(&image));
}

/// A font previews as a sample of its letters, drawn by magick.
///
/// Given: a TrueType font of this machine, under a name that says nothing about its kind.
/// When: it is previewed on a machine with magick.
/// Then: the preview is a picture of a font, with dark letters on a light ground.
///
/// Why it matters: a font file is told apart by how its letters look, and its tables say nothing
/// a person reads.
#[test]
fn a_font_previews_as_a_sample_of_its_letters() {
    let candidates = [
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/System/Library/Fonts/Geneva.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    let Some(font) = candidates.iter().map(Path::new).find(|path| path.is_file()) else {
        eprintln!("skipped: no TrueType font at a known path");
        return;
    };
    if !installed("magick") {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("face");
    fs::copy(font, &path).unwrap();

    // When:
    let image = rendered(&path, Kind::Font);

    // Then:
    let luma = image.to_luma8();
    assert!(
        luma.pixels().any(|pixel| pixel.0[0] < 64),
        "no dark letters"
    );
    assert!(
        luma.pixels().any(|pixel| pixel.0[0] > 192),
        "no light ground"
    );
}

/// A kind whose tool is missing falls back to the preview it had, with a line that names the
/// tool.
///
/// Given: a file of each kind, and a search path with no tools on it.
/// When: each is previewed.
/// Then: each previews as its contents, binary for four and text for the SVG, and the reason
/// names its tool.
///
/// Why it matters: every tool is optional, and an operator who sees only bytes needs to know
/// what to install to see the picture.
#[test]
fn a_kind_whose_tool_is_missing_falls_back_naming_the_tool() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let nothing = tempfile::tempdir().unwrap();
    let renderer = Renderer::searching(nothing.path());
    let box_head = |brand: &[u8]| [&[0, 0, 0, 0x18][..], b"ftyp", brand, &[0; 32]].concat();
    let cases: [(&str, Vec<u8>, &str); 5] = [
        ("report.pdf", b"%PDF-1.4\n%\x00\x01\n".to_vec(), "pdftoppm"),
        ("clip.mp4", box_head(b"isom"), "ffmpegthumbnailer"),
        (
            "drawing.svg",
            b"<?xml version=\"1.0\"?>\n<svg width=\"1\" height=\"1\"/>\n".to_vec(),
            "magick",
        ),
        ("photo.heic", box_head(b"heic"), "magick"),
        ("face.ttf", [&[0, 1, 0, 0][..], &[0; 32]].concat(), "magick"),
    ];

    for (name, contents, tool) in cases {
        let path = root.path().join(name);
        fs::write(&path, &contents).unwrap();

        // When:
        let shown = look(&path, &renderer);

        // Then:
        let Preview::Undecoded { reason, contents } = shown else {
            panic!("{name} falls back to its contents, not {shown:?}");
        };
        assert!(reason.contains(tool), "{name}: {reason}");
        if name.ends_with(".svg") {
            assert!(
                matches!(*contents, Preview::Text(_)),
                "{name}: {contents:?}"
            );
        } else {
            assert!(
                matches!(*contents, Preview::Binary(_)),
                "{name}: {contents:?}"
            );
        }
    }
}

/// A tool that runs too long is killed, and the preview says so.
///
/// Given: a PDF, and a pdftoppm that never finishes.
/// When: it is previewed.
/// Then: the preview falls back with a reason that names the tool and the time limit, it comes
/// back soon after that limit, and the tool and the process it started are gone.
///
/// Why it matters: the file is untrusted, and a tool stuck on it must not hold a process and a
/// thread for as long as the selector stays open.
#[test]
fn a_tool_that_runs_too_long_is_killed() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("report.pdf");
    pdf(&path);
    let (tools, record) = slow_tool("pdftoppm");
    let renderer = Renderer::searching(tools.path());
    let began = Instant::now();

    // When:
    let shown = look(&path, &renderer);

    // Then:
    let took = began.elapsed();
    assert!(took < TOOL_TIMEOUT + Duration::from_secs(2), "{took:?}");
    let Preview::Undecoded { reason, .. } = shown else {
        panic!("a stuck tool falls back, not {shown:?}");
    };
    assert!(reason.contains("pdftoppm"), "{reason}");
    assert!(
        reason.contains(&format!("{} s", TOOL_TIMEOUT.as_secs())),
        "{reason}"
    );
    let (pids, _) = started(&record);
    assert_gone(pids);
}

/// A tool is killed when the cursor leaves its file.
///
/// Given: a PDF, a pdftoppm that never finishes, and a preview of the PDF that has started it.
/// When: the stop that a cursor move sends is set.
/// Then: the preview returns nothing within a second, and the tool and the process it started
/// are gone.
///
/// Why it matters: a cursor that passes a folder of PDFs would otherwise leave a tool running
/// for every one it passed.
#[test]
fn a_tool_is_killed_when_the_cursor_moves_on() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("report.pdf");
    pdf(&path);
    let (tools, record) = slow_tool("pdftoppm");
    let renderer = Renderer::searching(tools.path());
    let stop = AtomicBool::new(false);

    let (shown, after_stop) = thread::scope(|scope| {
        let reading = scope.spawn(|| preview(&path, &Guard::for_host(), &renderer, &stop));
        let (pids, _) = started(&record);

        // When:
        stop.store(true, Ordering::Relaxed);
        let stopped = Instant::now();
        let shown = reading.join().unwrap();
        let after_stop = stopped.elapsed();

        // Then:
        assert_gone(pids);
        (shown, after_stop)
    });
    assert!(shown.is_none(), "{shown:?}");
    assert!(after_stop < Duration::from_secs(1), "{after_stop:?}");
}

/// Closing the renderer kills its tool and removes the directory the tool wrote in.
///
/// Given: a PDF, a pdftoppm that never finishes, and a preview of the PDF that has started it.
/// When: the renderer is closed, as the selector closes it.
/// Then: the preview returns nothing, the tool and the process it started are gone, and its
/// directory is gone.
///
/// Why it matters: a process left behind outlives blank, and a directory left behind holds
/// renders of files the operator went on to destroy.
#[test]
fn closing_the_renderer_kills_its_tool_and_removes_its_directory() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("report.pdf");
    pdf(&path);
    let (tools, record) = slow_tool("pdftoppm");
    let renderer = Renderer::searching(tools.path());
    let stop = AtomicBool::new(false);

    thread::scope(|scope| {
        let reading = scope.spawn(|| preview(&path, &Guard::for_host(), &renderer, &stop));
        let (pids, dir) = started(&record);
        assert!(dir.is_dir(), "the tool writes in {dir:?}");

        // When:
        renderer.close();

        // Then:
        assert_gone(pids);
        assert!(!dir.exists(), "{dir:?} is left behind");
        assert!(reading.join().unwrap().is_none());
    });
}

/// An SVG that claims a huge canvas is refused at once, and never drawn.
///
/// Given: an SVG of a few hundred bytes that claims 60000 by 60000 pixels.
/// When: it is previewed on a machine with magick.
/// Then: the preview falls back within two seconds, with the reason magick gave.
///
/// Why it matters: magick's own limits allow most of the machine's memory, and without bounds of
/// blank's this file takes gigabytes and the whole time limit.
#[test]
fn an_svg_that_claims_a_huge_canvas_is_refused_at_once() {
    if !installed("magick") {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("bomb.svg");
    let side = 60_000;
    let text = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{side}\" height=\"{side}\"><rect width=\"{side}\" height=\"{side}\" fill=\"red\"/></svg>\n"
    );
    fs::write(&path, text).unwrap();
    let began = Instant::now();

    // When:
    let shown = look(&path, &Renderer::for_host());

    // Then:
    let took = began.elapsed();
    let Preview::Undecoded { reason, .. } = shown else {
        panic!("a huge canvas falls back, not {shown:?}");
    };
    assert!(reason.contains("magick failed"), "{reason}");
    assert!(took < Duration::from_secs(2), "{took:?}");
}

/// A text file that starts like a video or a font header previews as text, with no tool.
///
/// Given: a text file that starts with `G` and holds another `G` 188 bytes on, as a transport
/// stream does, and a text file that starts with `true`, as an old TrueType font does.
/// When: each is previewed with no tools on the search path.
/// Then: each previews as its text, with no line about a tool.
///
/// Why it matters: a tool run on a file that is not its kind fails, and the pane would show an
/// operator a failure line above an ordinary note.
#[test]
fn a_text_file_that_starts_like_a_media_header_previews_as_text() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let nothing = tempfile::tempdir().unwrap();
    let renderer = Renderer::searching(nothing.path());
    let mut stream = "G".to_owned();
    stream.push_str(&"a".repeat(187));
    stream.push_str("G and the rest of the note\n");
    let cases = [
        ("stream.txt", stream),
        ("flags.txt", "true\nfalse\n".to_owned()),
    ];

    for (name, text) in cases {
        let path = root.path().join(name);
        fs::write(&path, text).unwrap();

        // When:
        let shown = look(&path, &renderer);

        // Then:
        assert!(matches!(shown, Preview::Text(_)), "{name}: {shown:?}");
    }
}

/// An SVG cannot read a file through a coder that blank does not allow.
///
/// Given: a text note, and an SVG that draws the note as an image by its path.
/// When: the SVG is previewed on a machine with magick.
/// Then: the preview falls back, and the reason says the security policy refused it.
///
/// Why it matters: an SVG reaches every coder magick has through a path to a file planted beside
/// it, and some coders read text, run drawing scripts, or write and delete files.
#[test]
fn an_svg_cannot_read_a_file_through_a_coder_blank_does_not_allow() {
    if !installed("magick") {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let note = root.path().join("notes.txt");
    fs::write(&note, "a line of a private note\n").unwrap();
    let path = root.path().join("drawing.svg");
    let text = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"300\" height=\"100\"><image xlink:href=\"{}\" width=\"300\" height=\"100\"/></svg>\n",
        note.display()
    );
    fs::write(&path, text).unwrap();

    // When:
    let shown = look(&path, &Renderer::for_host());

    // Then:
    let Preview::Undecoded { reason, .. } = shown else {
        panic!("an SVG that reads a note falls back, not {shown:?}");
    };
    assert!(reason.contains("security policy"), "{reason}");
}
