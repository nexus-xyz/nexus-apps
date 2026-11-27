//! A recording from the shortcut to the folder, on macOS: start `screencapture`, show the ink overlay, sample the
//! pointer and watch the app in front; on stop, make the folder and copy it for the agent. The shell's part (the
//! menu-bar item, its main thread, the panel, the policy) goes through [`Shell`]. Also `--process-recording` and
//! `--frames` ([`cli`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::gesture::{Draw, Gestures};
use super::model::{Capture, Display, Event};
use super::overlay::{self, Faded};
use super::screen::{self, Screen};
use super::screencapture::Recording;
use super::summary::{clipboard, clock, parse_time};
use super::{Config, Shell, process, system};

/// How long ink and pins stay before they fade, and how long the fade takes (seconds).
const INK_STAYS: f64 = 2.5;
const PIN_STAYS: f64 = 6.0;
const FADE: f64 = 0.7;

struct Session {
    dir: PathBuf,
    video: Recording,
    began: Instant,
    capture: Capture,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<Vec<Event>>>,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Starting,
    Recording(Box<Session>),
    Processing,
}

struct Inner {
    phase: Mutex<Phase>,
    shell: Arc<dyn Shell>,
    config: Config,
}

/// The recording in progress, if any. One per app; clones share it.
#[derive(Clone)]
pub struct Recorder {
    inner: Arc<Inner>,
}

impl Recorder {
    pub fn new(shell: Arc<dyn Shell>, config: Config) -> Recorder {
        Recorder {
            inner: Arc::new(Inner {
                phase: Mutex::new(Phase::Idle),
                shell,
                config,
            }),
        }
    }

    fn phase(&self) -> MutexGuard<'_, Phase> {
        self.inner.phase.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn shell(&self) -> &dyn Shell {
        self.inner.shell.as_ref()
    }

    fn log(&self) -> &'static str {
        self.inner.config.log
    }

    pub fn is_recording(&self) -> bool {
        matches!(*self.phase(), Phase::Recording(_))
    }

    /// Show `title` for `seconds`, then nothing, unless something else took the item meanwhile.
    fn flash(&self, title: &str, seconds: u64) {
        self.shell().status(Some(title));
        let this = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(seconds));
            if matches!(*this.phase(), Phase::Idle) {
                this.shell().status(None);
            }
        });
    }

    /// The shortcut, the menu-bar item and the panel's button: start a recording, or stop the one in progress.
    pub fn toggle(&self) {
        if !super::available(self.shell().allowed()) {
            self.flash("Recording is off", 4);
            return;
        }
        let mut current = self.phase();
        match std::mem::take(&mut *current) {
            Phase::Idle => {
                *current = Phase::Starting;
                drop(current);
                let this = self.clone();
                std::thread::spawn(move || this.start());
            }
            Phase::Recording(session) => {
                *current = Phase::Processing;
                drop(current);
                let this = self.clone();
                std::thread::spawn(move || this.finish(*session));
            }
            other => *current = other,
        }
    }

    /// Stop the recording in progress and keep nothing.
    pub fn cancel(&self) {
        let mut current = self.phase();
        let Phase::Recording(session) = std::mem::take(&mut *current) else {
            return;
        };
        drop(current);
        let session = *session;
        session.stop.store(true, Ordering::SeqCst);
        self.shell().on_main(Box::new(overlay::hide));
        // Off the caller's thread (a menu's): the threads take up to a couple of seconds to notice.
        let this = self.clone();
        std::thread::spawn(move || {
            session.video.cancel();
            for thread in session.threads {
                let _ = thread.join();
            }
            let _ = std::fs::remove_dir_all(&session.dir);
            this.shell().status(None);
        });
    }

    /// After `seconds`, on the main thread, `then`.
    fn later(&self, seconds: f64, then: impl FnOnce() + Send + 'static) {
        let this = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs_f64(seconds));
            this.shell().on_main(Box::new(then));
        });
    }

    /// Fade each finished stroke and pin a while after it is drawn.
    fn schedule_fades(&self, draws: &[Draw]) {
        for draw in draws {
            let (target, stays) = match draw {
                Draw::End { id } => (Faded::Stroke(*id), INK_STAYS),
                Draw::Pin { n, .. } => (Faded::Pin(*n), PIN_STAYS),
                _ => continue,
            };
            self.later(stays, move || overlay::fade(target));
            self.later(stays + FADE, move || overlay::remove(target));
        }
    }

    /// The pointer, Shift and the button, about 60 times a second: ink on the overlay, events for the timeline.
    fn sampler(&self, screen: Screen, began: Instant, stop: Arc<AtomicBool>) -> JoinHandle<Vec<Event>> {
        let this = self.clone();
        std::thread::spawn(move || {
            let mut gestures = Gestures::default();
            let mut events = Vec::new();
            screen::sample(screen, began, stop, |sample| {
                let (draws, done) = gestures.feed(sample);
                if !draws.is_empty() {
                    this.schedule_fades(&draws);
                    this.shell().on_main(Box::new(move || overlay::apply(draws)));
                }
                events.extend(done);
            });
            events.extend(gestures.finish(began.elapsed().as_secs_f64()));
            events
        })
    }

    fn start(&self) {
        match self.begin() {
            Ok(session) => {
                *self.phase() = Phase::Recording(Box::new(session));
                self.shell().status(Some("● 0:00"));
                self.tick();
            }
            Err(error) => {
                *self.phase() = Phase::Idle;
                eprintln!("{}: recording: {error}", self.log());
                // Almost always the Screen Recording permission: show where it is granted.
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
                    .status();
                self.flash("Allow screen recording", 8);
            }
        }
    }

    fn begin(&self) -> Result<Session, String> {
        let screen = screen::under_pointer().ok_or("no display to record")?;
        let root = self.inner.config.folder.clone().ok_or("no data folder")?;
        let (dir, created_at) = new_folder(&root)?;
        let began = Instant::now();
        let video = match Recording::start(screen.index, &dir.join(process::VIDEO)) {
            Ok(video) => video,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(error);
            }
        };
        let mut capture = Capture::new(
            created_at,
            self.inner.config.app.clone(),
            Display {
                index: screen.index,
                width: screen.bounds.size.width,
                height: screen.bounds.size.height,
            },
        );
        capture.narration = video.narration;
        self.shell().hide_panel();
        self.shell().on_main(Box::new(move || overlay::show(screen)));
        let stop = Arc::new(AtomicBool::new(false));
        let threads = vec![self.sampler(screen, began, stop.clone()), watcher(began, stop.clone())];
        Ok(Session {
            dir,
            video,
            began,
            capture,
            stop,
            threads,
        })
    }

    /// The recording's time beside the menu-bar icon, every second; stops it if `screencapture` stopped by itself.
    fn tick(&self) {
        let this = self.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let mut current = this.phase();
                let Phase::Recording(session) = &mut *current else {
                    return;
                };
                if !session.video.running() {
                    drop(current);
                    eprintln!("{}: recording: screencapture stopped by itself", this.log());
                    this.toggle();
                    return;
                }
                let title = format!("● {}", clock(session.began.elapsed().as_secs_f64()));
                drop(current);
                this.shell().status(Some(&title));
            }
        });
    }

    fn finish(&self, session: Session) {
        let Session {
            dir,
            video,
            began,
            mut capture,
            stop,
            threads,
        } = session;
        let stopped_at = began.elapsed().as_secs_f64();
        stop.store(true, Ordering::SeqCst);
        self.shell().status(Some("Processing…"));
        let stopped = video.stop();
        self.shell().on_main(Box::new(overlay::hide));
        for thread in threads {
            capture.events.extend(thread.join().unwrap_or_default());
        }
        capture.events.sort_by(|a, b| a.t().total_cmp(&b.t()));
        capture.stopped_at = Some(stopped_at);
        let result = stopped
            .and_then(|()| process::write(&dir, &capture))
            .and_then(|()| process::process(&dir, &mut capture, true));
        *self.phase() = Phase::Idle;
        match result {
            Ok(sheet) => {
                link_latest(&dir);
                let text = clipboard(&capture, &dir);
                self.shell().on_main(Box::new(move || system::copy(&sheet, &text)));
                eprintln!("{}: recording: {}", self.log(), dir.display());
                self.flash("Copied for your agent", 5);
            }
            Err(error) => {
                eprintln!("{}: recording: {error} ({})", self.log(), dir.display());
                self.flash("Recording failed", 8);
            }
        }
    }
}

/// The app in front, twice a second, and a browser's address when it changes (checked every four seconds).
fn watcher(began: Instant, stop: Arc<AtomicBool>) -> JoinHandle<Vec<Event>> {
    std::thread::spawn(move || {
        let mut events = Vec::new();
        let mut last: Option<(system::Front, Option<String>)> = None;
        let mut checked = Instant::now();
        while !stop.load(Ordering::SeqCst) {
            if let Some(front) = system::frontmost() {
                let changed = last.as_ref().is_none_or(|(previous, _)| *previous != front);
                let browser = system::is_browser(front.bundle_id.as_deref());
                if changed || (browser && checked.elapsed() >= Duration::from_secs(4)) {
                    checked = Instant::now();
                    let url = front.bundle_id.as_deref().and_then(system::browser_url);
                    if changed || last.as_ref().is_some_and(|(_, previous)| *previous != url) {
                        events.push(Event::App {
                            t: began.elapsed().as_secs_f64(),
                            name: front.name.clone(),
                            bundle_id: front.bundle_id.clone(),
                            url: url.clone(),
                        });
                        last = Some((front, url));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        events
    })
}

/// `2026-10-19T14-03-22` for the folder, `2026-10-19T14:03:22-0600` for `capture.json` (local time, from `date`).
fn stamp() -> (String, String) {
    let output = std::process::Command::new("/bin/date")
        .arg("+%Y-%m-%dT%H-%M-%S|%Y-%m-%dT%H:%M:%S%z")
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default();
    match output.split_once('|') {
        Some((name, created)) => (name.to_string(), created.to_string()),
        None => {
            let seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or_default();
            (seconds.to_string(), seconds.to_string())
        }
    }
}

fn new_folder(root: &Path) -> Result<(PathBuf, String), String> {
    let (name, created) = stamp();
    std::fs::create_dir_all(root).map_err(|error| format!("{}: {error}", root.display()))?;
    for attempt in 1..100 {
        let dir = if attempt == 1 {
            root.join(&name)
        } else {
            root.join(format!("{name}-{attempt}"))
        };
        if std::fs::create_dir(&dir).is_ok() {
            return Ok((dir, created));
        }
    }
    Err(format!("could not make a folder in {}", root.display()))
}

/// `recordings/latest` → the newest recording.
fn link_latest(dir: &Path) {
    let (Some(root), Some(name)) = (dir.parent(), dir.file_name()) else {
        return;
    };
    let latest = root.join("latest");
    let _ = std::fs::remove_file(&latest);
    let _ = std::os::unix::fs::symlink(name, latest);
}

fn arg_after(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|at| args.get(at + 1))
        .cloned()
}

/// `<binary> --process-recording <folder>` (make the frames, transcript and summary again; prints what the clipboard
/// would get) and `<binary> --frames <folder> --from 0:10 --to 0:14 [--count 8]` (full-resolution frames for a closer
/// look; prints their paths). `app` names the shell in a new `capture.json`, `log` prefixes its errors. `None` when
/// neither was asked for.
pub fn cli(app: &str, log: &str) -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(dir) = arg_after(&args, "--process-recording") {
        let dir = PathBuf::from(dir);
        let mut capture = process::read(&dir).unwrap_or_else(|_| {
            let mut capture = Capture::new(stamp().1, app.to_string(), Display::default());
            capture.narration = true;
            capture
        });
        return Some(match process::process(&dir, &mut capture, false) {
            Ok(_) => {
                print!("{}", clipboard(&capture, &dir));
                0
            }
            Err(error) => {
                eprintln!("{log}: {error}");
                1
            }
        });
    }
    let dir = arg_after(&args, "--frames")?;
    let time = |name: &str| arg_after(&args, name).and_then(|text| parse_time(&text));
    let (Some(from), Some(to)) = (time("--from"), time("--to")) else {
        eprintln!("{log}: --frames <folder> --from <time> --to <time> [--count <n>] (times as 0:12 or 12.5)");
        return Some(2);
    };
    let count = arg_after(&args, "--count")
        .and_then(|text| text.parse().ok())
        .unwrap_or(8);
    Some(match process::frames_between(Path::new(&dir), from, to, count) {
        Ok(paths) => {
            for path in paths {
                println!("{}", path.display());
            }
            0
        }
        Err(error) => {
            eprintln!("{log}: {error}");
            1
        }
    })
}
