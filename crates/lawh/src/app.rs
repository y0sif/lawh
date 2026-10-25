//! The window: winit events in, frames drawn by lawh-render out through softbuffer.
//!
//! Keys go to the editing engine ([`Engine`]), which changes the board; the app does what the
//! engine asks of it ([`Effect`]): it writes the board, exports it, quits, and times out keys
//! waiting for a longer mapping. Before each key it tells the engine what part of the board the
//! window shows, for jump labels. Each frame draws the board with the engine's cursor,
//! selection, caret, jump labels, `:sym` rows and status line over it. The mouse pans (drag),
//! zooms (wheel) and selects (click).
//!
//! An export runs on this thread and may take seconds on a hostile board, so the window first
//! shows that it starts (see [`Suspended`]).

use std::collections::VecDeque;
use std::io;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow};
use lawh_core::{Cursor, Effect, Engine, ExportOptions, Key, Mode, ObjectId, Point, Rect, to_lawh};
use lawh_render::{
    Caret, FrameStats, Overlay, OverlayCursor, Renderer, StatusLine, Viewport, status_bar_height_px,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

use crate::{export, files, keys, status, view};

/// Window size on open, in logical pixels.
const INITIAL_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 800.0);

/// Furthest the pointer may move, in logical pixels, between pressing and releasing the left
/// button for it to be a click (select) and not a drag (pan). A hand never holds a mouse
/// perfectly still; past this the press pans.
const CLICK_SLOP_PX: f64 = 4.0;

/// How near an object, in logical pixels, a click still selects it, so a thin line is easy to
/// click.
const HIT_TOLERANCE_PX: f64 = 4.0;

/// The status line on a close request while the board has unwritten changes. A second close
/// request closes while this still shows: a key, a click on the board or a mapping timeout
/// between drops it, and the next close warns again.
const CLOSE_MODIFIED: &str = "unsaved changes: :w to save, :q! to discard, or close again";

/// Longest an export waits for the frame that shows it starting (`exporting "out.png"` on the
/// status line) before it runs anyway. The frame normally comes within a refresh (17 ms at
/// 60 Hz), but a window that isn't shown may get none: on Wayland winit holds a redraw until the
/// compositor's frame callback for the last frame, which a hidden surface never gets. Short, so
/// that an export typed just before the window was minimized still runs at once.
const EXPORT_FRAME_WAIT: Duration = Duration::from_millis(250);

/// The status line while an export waits to run (see [`Suspended`]).
fn exporting(path: &str) -> String {
    format!("exporting \"{path}\"")
}

/// Sent to the event loop by the math engine's waker when a formula finishes compiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MathReady;

/// The open board and its window.
pub struct App {
    engine: Engine,
    /// The file the board was opened from, as the engine names it and as the system does (the
    /// two differ only for a path that isn't UTF-8).
    file: Option<(String, PathBuf)>,
    /// The directory lawh started in: `:w` takes a relative path from here.
    launch_dir: Option<PathBuf>,
    /// The user's home directory: `:w ~/a.lawh` writes `a.lawh` in it.
    home: Option<PathBuf>,
    /// The window title, kept to set it only when it changes.
    title: String,
    /// Draws the board and measures it for the engine; its math engine never blocks a frame,
    /// and its work budget keeps frames short whatever the board holds.
    renderer: Renderer,
    /// Created on the first `resumed`.
    gfx: Option<Gfx>,
    /// `None` until the next frame fits the board in the window; after that it holds the
    /// user's pan and zoom.
    viewport: Option<Viewport>,
    /// The view no longer fits the board again when a formula finishes (its real size replacing
    /// its placeholder's): the user has panned, zoomed, clicked or typed a key.
    keep_view: bool,
    /// Last pointer position in physical pixels, once the pointer has been over the window.
    pointer: Option<Point>,
    /// The left button, while it is held.
    left: Option<LeftPress>,
    middle_down: bool,
    modifiers: ModifiersState,
    /// The engine's timer for keys waiting for a longer mapping.
    timer: Timer,
    /// The app's own message for the status line (a lawhrc warning, a refused close), until
    /// the next key, click or timeout. While it is [`CLOSE_MODIFIED`], a close request closes
    /// ([`App::close_or_warn`]).
    message: Option<String>,
    /// The engine asked to quit: keys still queued are dropped.
    quitting: bool,
    /// An export the engine asked for, waiting for a frame to show that it starts.
    suspended: Option<Suspended>,
    /// When lawh started, for the first-frame log.
    started: Instant,
    frames: u64,
    /// A failure that ended the event loop.
    error: Option<anyhow::Error>,
}

/// The window and the surface frames are presented on.
struct Gfx {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    /// Size the surface was last resized to.
    surface_size: Option<(NonZeroU32, NonZeroU32)>,
}

/// The left mouse button, while it is held.
#[derive(Debug, Clone, Copy)]
struct LeftPress {
    /// Where it went down, in physical pixels.
    at: Point,
    /// It moved past [`CLICK_SLOP_PX`]: it pans, and its release is no click.
    dragging: bool,
}

/// An export ([`Effect::Export`]) held back until a frame has shown that it starts.
///
/// An export runs on the event loop's thread, and a hostile board makes it take seconds (its
/// caps are lawh-render's), so the window says so first. When the engine asks for one, the app
/// stops doing the key's effects there: it keeps the export, the effects after it and the keys
/// not typed into the engine yet, shows `exporting "out.png"` on the status line, and asks for a
/// frame. Once that frame is presented, the event loop goes round once without waiting, so the
/// frame leaves the app before the thread is busy (winit's X11 loop flushes the requests at the
/// start of each turn, and softbuffer's Wayland present flushes them itself; macOS and Windows
/// are untested), and the export runs at the start of the next turn ([`App::run_export`]), then
/// the effects and keys that waited, in order. So everything happens in the order the engine
/// asked for it, and the board doesn't change before the export draws it.
///
/// Meanwhile keys are queued behind the ones waiting. A click, a close request, or a failure
/// that ends the app does not wait for frames: it first runs every export waiting and all that
/// waited with them, the exports those keys ask for in turn too, in the engine's order
/// ([`App::finish_exports`]), then itself. So a click lands on the board those keys leave (at
/// the point the user clicked, through the view they saw), and closing or failing drops no
/// export or write typed before it; their report is not lost to the click or to the close
/// warning ([`App::click`], [`App::close`]). If no frame comes within
/// [`EXPORT_FRAME_WAIT`] (a hidden window), the export runs without one.
#[derive(Debug)]
struct Suspended {
    job: ExportJob,
    /// The effects of the export's key after the export, and that key's writes so far.
    effects: Effects,
    /// Keys not typed into the engine yet: those after the export's key in its batch, then the
    /// keys typed since, in order.
    keys: VecDeque<Key>,
    /// A frame showing the export has been presented, or the window has no area to show one.
    shown: bool,
    /// When the export runs even without that frame.
    deadline: Instant,
}

impl Suspended {
    /// The export is to run at `now`: its frame has been presented, or waited for long enough.
    fn due(&self, now: Instant) -> bool {
        self.shown || now >= self.deadline
    }
}

/// An export the engine asked for ([`Effect::Export`]), with the file it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExportJob {
    /// The file as the engine named it, for [`Engine::export_done`].
    path: String,
    /// The file it writes ([`files::write_target`]).
    target: PathBuf,
    options: ExportOptions,
    selection: Option<Vec<ObjectId>>,
}

/// The one timer the engine may ask for ([`Effect::ArmTimer`]).
#[derive(Debug, Default)]
struct Timer {
    deadline: Option<Instant>,
}

impl Timer {
    /// Arms the timer `ms` milliseconds after `now`, in place of any armed before.
    fn arm(&mut self, now: Instant, ms: u64) {
        self.deadline = Some(now.checked_add(Duration::from_millis(ms)).unwrap_or(now));
    }

    fn cancel(&mut self) {
        self.deadline = None;
    }

    /// True once, when the deadline has passed at `now`; the timer is then disarmed.
    fn fire(&mut self, now: Instant) -> bool {
        self.deadline.take_if(|deadline| now >= *deadline).is_some()
    }

    /// How the event loop waits: until the deadline, or for the next event.
    fn control_flow(&self) -> ControlFlow {
        self.deadline
            .map_or(ControlFlow::Wait, ControlFlow::WaitUntil)
    }
}

impl App {
    /// The app for `engine`, whose board was opened from `file`, with relative `:w` paths taken
    /// from `launch_dir` and `~` from the user's home directory (see [`files::write_target`]),
    /// and `message` on the status line until the first key.
    ///
    /// The cursor starts on the board's first object in reading order ([`Engine::go_first`];
    /// an empty board keeps the grid's origin), which the opening view shows
    /// ([`view::opening`]). Call it after the lawhrc is read, as its settings may matter.
    pub fn new(
        mut engine: Engine,
        file: Option<PathBuf>,
        launch_dir: Option<PathBuf>,
        mut renderer: Renderer,
        started: Instant,
        message: Option<String>,
    ) -> Self {
        engine.go_first(&mut renderer);
        let file = engine.file_name().map(str::to_owned).zip(file);
        Self {
            title: status::title(&engine),
            engine,
            file,
            launch_dir,
            home: files::home_dir(),
            renderer,
            gfx: None,
            viewport: None,
            keep_view: false,
            pointer: None,
            left: None,
            middle_down: false,
            modifiers: ModifiersState::empty(),
            timer: Timer::default(),
            message,
            quitting: false,
            suspended: None,
            started,
            frames: 0,
            error: None,
        }
    }

    /// Why the event loop ended early, if it did.
    pub fn into_result(self) -> anyhow::Result<()> {
        self.error.map_or(Ok(()), Err)
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, err: anyhow::Error) {
        self.failed(err);
        event_loop.exit();
    }

    /// A failure that ends the event loop, such as a frame that could not be presented: logged
    /// and kept for [`App::into_result`], once every export waiting and what waited with it have
    /// run ([`App::finish_exports`]). Keys typed before the failure did all they asked for before
    /// exports waited for frames, so they still do: `:exp a.png<CR>:w<CR>` gets its file and its
    /// write.
    fn failed(&mut self, err: anyhow::Error) {
        tracing::error!("{err:#}");
        self.error.get_or_insert(err);
        self.finish_exports();
    }

    fn request_redraw(&self) {
        if let Some(gfx) = &self.gfx {
            gfx.window.request_redraw();
        }
    }

    fn scale_factor(&self) -> f64 {
        self.viewport.map_or(1.0, |vp| vp.scale_factor)
    }

    /// A key press, not a release or a synthetic one.
    fn key(&mut self, event_loop: &ActiveEventLoop, event: &KeyEvent) {
        if self.quitting {
            return;
        }
        let text_mode = matches!(self.engine.mode(), Mode::Insert | Mode::CommandLine);
        let translated = keys::translate(&event.logical_key, self.modifiers, text_mode);
        if translated.reset_dead_keys
            && let Some(gfx) = &self.gfx
        {
            gfx.window.reset_dead_keys();
        }
        // A modifier alone is no key: it must not cut short the engine's timer.
        if translated.keys.is_empty() {
            return;
        }
        // Only macOS has Cmd+Q as its quit key; elsewhere Super+Q is the desktop's, and goes to
        // the engine as `<D-q>`.
        if cfg!(target_os = "macos") && keys::is_close_request(&translated.keys) {
            // A held Cmd+Q repeats: only the press counts, so holding it can't skip the
            // unsaved-changes warning. Like the close button, it leaves the timer and the
            // message alone.
            if !event.repeat {
                self.close_requested(event_loop);
            }
            return;
        }
        self.timer.cancel();
        self.message = None;
        self.keep_view = true;
        self.feed(translated.keys);
        if self.quitting {
            event_loop.exit();
        }
    }

    /// Types `keys` into the engine ([`App::type_keys`]), or queues them behind an export
    /// waiting to run ([`Suspended`]).
    fn feed(&mut self, keys: Vec<Key>) {
        if let Some(suspended) = &mut self.suspended {
            suspended.keys.extend(keys);
            return;
        }
        self.sync_view();
        self.type_keys(keys.into());
    }

    /// Types `keys` into the engine, one by one, doing what each asks for ([`App::apply`]),
    /// then [`App::after_input`]. Each key drops the armed timer first, as the engine asks
    /// ([`Effect::ArmTimer`]). Keys left once the engine quits are dropped; keys left after a
    /// key that asks for an export wait for it ([`Suspended`]).
    fn type_keys(&mut self, mut keys: VecDeque<Key>) {
        loop {
            if self.quitting {
                return;
            }
            if let Some(suspended) = &mut self.suspended {
                suspended.keys.extend(keys);
                break;
            }
            let Some(key) = keys.pop_front() else {
                break;
            };
            self.timer.cancel();
            let effects = self.engine.feed(key, &mut self.renderer);
            self.apply(Effects::new(effects));
        }
        self.after_input();
    }

    /// The engine's timer went off: keys waiting for a longer mapping run now. As a key does, it
    /// drops the app's message, a close warning too. (No timer is armed while an export waits:
    /// the timer its key asked for waits with it.)
    fn timeout(&mut self) {
        self.message = None;
        self.sync_view();
        let effects = self.engine.timeout(&mut self.renderer);
        self.apply(Effects::new(effects));
        if !self.quitting {
            self.after_input();
        }
    }

    /// Tells the engine which part of the board the window shows above the status bar
    /// ([`view::board_area`]): `s` labels the objects there.
    ///
    /// Labels are made only while the engine takes a key or a timeout, so the app calls this
    /// right before each, from the view as it is then: whatever panned, zoomed, resized or
    /// followed the cursor since, the engine has the view the user sees. Labels already showing
    /// stay on their objects when the view moves. Until the first frame, and after a formula
    /// finishing fits the board to the window again, there is no view yet: the next frame shows
    /// the whole board, so every object counts as in view (`None`).
    fn sync_view(&mut self) {
        let area = self
            .viewport
            .map(|vp| view::board_area(&vp, f64::from(status_bar_height_px(vp.scale_factor))));
        self.engine.set_view(area);
    }

    /// Does what the engine asked for, in order ([`Effects::run`]): the writes, then quitting
    /// ([`App::quitting`], after which the caller leaves the event loop) or arming the timer.
    /// An export stops it there: the export and the effects after it wait for a frame that
    /// shows it ([`Suspended`]).
    fn apply(&mut self, mut effects: Effects) {
        let opened = self
            .file
            .as_ref()
            .map(|(name, real)| (name.as_str(), real.as_path()));
        let launch_dir = self.launch_dir.as_deref();
        let home = self.home.as_deref();
        let job = effects.run(
            &mut self.engine,
            |path| files::write_target(path, opened, launch_dir, home),
            write_board,
            files::same_file,
        );
        if let Some(job) = job {
            tracing::info!(path = %job.target.display(), "export waits for its frame");
            let now = Instant::now();
            self.suspended = Some(Suspended {
                job,
                effects,
                keys: VecDeque::new(),
                shown: false,
                deadline: now.checked_add(EXPORT_FRAME_WAIT).unwrap_or(now),
            });
            return;
        }
        if effects.after.quit {
            tracing::info!("quitting");
            self.quitting = true;
            self.timer.cancel();
        } else if let Some(ms) = effects.after.timer_ms {
            self.timer.arm(Instant::now(), ms);
        }
    }

    /// Runs the export waiting for its frame ([`Suspended`]), if there is one: draws and
    /// writes it with the window's renderer ([`export::export_to`]), reports it to the engine
    /// ([`Engine::export_done`]), then does the effects that came after it and types the keys
    /// that waited, as if it had run when the engine asked for it. The caller leaves the event
    /// loop if that quits. One of those keys may ask for another export, which then waits for a
    /// frame of its own; [`App::finish_exports`] runs them all at once.
    fn run_export(&mut self) {
        let Some(Suspended {
            job, effects, keys, ..
        }) = self.suspended.take()
        else {
            return;
        };
        let result = export::export_to(
            &mut self.renderer,
            self.engine.board(),
            job.selection.as_deref(),
            job.options,
            &job.target,
        );
        if let Err(err) = &result {
            tracing::warn!(path = %job.target.display(), %err, "could not export");
        }
        self.engine.export_done(&job.path, result);
        self.apply(effects);
        self.sync_view();
        self.type_keys(keys);
    }

    /// Runs exports waiting ([`App::run_export`]) until none is left or the engine quits: the
    /// one waiting, then any the keys that waited with it ask for, and so on, with the effects
    /// and keys between them, in the engine's order. For what can't wait for their frames: a
    /// click, which must land on the board those keys leave, and a close request or a failure,
    /// which must not drop them.
    ///
    /// It ends: a new export can only come from the keys that waited (one key asks for at most
    /// one), so each turn leaves fewer of them. True when it ran one: the engine's message is
    /// then what they left, which no frame has shown yet.
    fn finish_exports(&mut self) -> bool {
        let mut ran = false;
        while self.suspended.is_some() && !self.quitting {
            self.run_export();
            ran = true;
        }
        ran
    }

    /// A frame was presented, or the window has nothing to present one on: an export waiting
    /// for its frame may run.
    fn frame_shown(&mut self) {
        if let Some(suspended) = &mut self.suspended {
            suspended.shown = true;
        }
    }

    /// How the event loop waits: not at all once an export's frame is presented (the export
    /// runs at the start of the next turn, [`ApplicationHandler::new_events`]), until
    /// [`EXPORT_FRAME_WAIT`] is up while it waits for that frame, else for the engine's timer
    /// or the next event. No timer is armed while an export waits.
    fn control_flow(&self) -> ControlFlow {
        match &self.suspended {
            Some(suspended) if suspended.shown => ControlFlow::Poll,
            Some(suspended) => ControlFlow::WaitUntil(suspended.deadline),
            None => self.timer.control_flow(),
        }
    }

    /// After keys: the title shows whether the board has changes, the view follows the cursor,
    /// and the frame is drawn again.
    fn after_input(&mut self) {
        self.update_title();
        if let Some(vp) = self.viewport.as_mut() {
            let focus = focus(&self.engine, &mut self.renderer);
            view::follow(vp, focus, f64::from(status_bar_height_px(vp.scale_factor)));
        }
        self.request_redraw();
    }

    fn update_title(&mut self) {
        let title = status::title(&self.engine);
        if title != self.title {
            if let Some(gfx) = &self.gfx {
                gfx.window.set_title(&title);
            }
            self.title = title;
        }
    }

    /// The window's close button, the system's close shortcut, or Cmd+Q on macOS
    /// ([`keys::is_close_request`]).
    fn close_requested(&mut self, event_loop: &ActiveEventLoop) {
        if self.close() {
            event_loop.exit();
        }
    }

    /// A close request: true when the window closes now. The exports waiting for their frames
    /// were asked for before the close, so they run first, with what waited with them
    /// ([`App::finish_exports`]); then the window closes if that quit (a `:wq` that waited).
    /// If they left an error (an export or a write that failed), the window stays open with
    /// that error on the status line, and the close is not armed: nobody has seen the error,
    /// and the close warning would hide it, as closing would. The next close request goes on as
    /// usual (the warning first if the board has changes). Else as [`App::close_or_warn`] says.
    fn close(&mut self) -> bool {
        let drained = self.finish_exports();
        if self.quitting {
            return true;
        }
        let failed = drained && self.engine.message().is_some_and(|m| m.error);
        if !failed && self.close_or_warn() {
            return true;
        }
        self.request_redraw();
        false
    }

    /// A close request: true when the window closes now ([`close_now`]), else the status line
    /// warns of the unwritten changes ([`CLOSE_MODIFIED`]).
    ///
    /// The close is armed only while that warning shows, so whatever drops it (a key, a click
    /// on the board, a mapping timeout) disarms it too, and the next close warns again. Panning,
    /// zooming and a click on the status bar leave it armed.
    fn close_or_warn(&mut self) -> bool {
        let warned = self.message.as_deref() == Some(CLOSE_MODIFIED);
        if close_now(self.engine.is_modified(), warned) {
            return true;
        }
        self.message = Some(CLOSE_MODIFIED.to_owned());
        false
    }

    fn pointer_moved(&mut self, to: Point) {
        let last = self.pointer.replace(to);
        let scale = self.scale_factor();
        let from = match self.left.as_mut() {
            Some(press) if !press.dragging => {
                if !past_click_slop(press.at, to, scale) {
                    return;
                }
                // The pan starts where the button went down, so the drag loses no distance.
                press.dragging = true;
                Some(press.at)
            }
            Some(_) => last,
            None if self.middle_down => last,
            None => return,
        };
        if let (Some(from), Some(vp)) = (from, self.viewport.as_mut()) {
            view::pan(vp, from, to);
            self.keep_view = true;
            self.request_redraw();
        }
    }

    /// A formula finished compiling: draw it, fitting the view again if the user hasn't moved
    /// it, clicked or typed a key yet.
    fn math_ready(&mut self) {
        if !self.keep_view && self.viewport.take().is_some() {
            tracing::debug!("math finished; fitting the board to the window again");
        }
        self.request_redraw();
    }

    fn mouse_input(&mut self, state: ElementState, button: MouseButton) {
        match (button, state) {
            (MouseButton::Left, ElementState::Pressed) => {
                self.left = self.pointer.map(|at| LeftPress {
                    at,
                    dragging: false,
                });
            }
            (MouseButton::Left, ElementState::Released) => {
                if let Some(press) = self.left.take()
                    && !press.dragging
                {
                    self.click(press.at);
                }
            }
            (MouseButton::Middle, state) => self.middle_down = state.is_pressed(),
            _ => {}
        }
    }

    /// A left click at `at` (physical pixels): the engine's cursor goes to the object there, or
    /// to the grid point nearest it. A click on the status bar does nothing.
    ///
    /// Every export waiting for its frame runs first, with the keys that waited
    /// ([`App::finish_exports`]), so the click lands on the board they leave. It lands where the
    /// user clicked, though: `at` becomes a board point through the view as it was when they
    /// clicked, before those keys move the view (it follows the cursor), and the view is put
    /// back, so what they clicked stays where they saw it. What those exports and keys left on
    /// the status line (an export's report, a failed write) stays after the click
    /// ([`Engine::restore_message`]): it comes just before the click, and nobody has seen it yet.
    fn click(&mut self, at: Point) {
        let seen = self.viewport;
        let clicked = seen.and_then(|vp| click_point(&vp, at));
        let drained = self.finish_exports();
        if self.quitting {
            return;
        }
        let Some((point, tolerance)) = clicked else {
            return;
        };
        if drained {
            self.viewport = seen;
        }
        let hit = self
            .renderer
            .hit_test(self.engine.board(), point, tolerance);
        tracing::debug!(?point, ?hit, "click");
        // The click drops keys waiting for a mapping, and with them their timer, and the app's
        // message: a close warning too, so the next close warns again.
        self.timer.cancel();
        self.message = None;
        // A formula finishing must not move the board from under what was just clicked.
        self.keep_view = true;
        let news = drained.then(|| self.engine.message().cloned()).flatten();
        self.engine.click(point, hit, &mut self.renderer);
        if let Some(news) = news {
            self.engine.restore_message(news);
        }
        self.update_title();
        self.request_redraw();
    }

    fn zoom(&mut self, factor: f64) {
        let Some(vp) = self.viewport.as_mut() else {
            return;
        };
        let at = self.pointer.unwrap_or(Point::new(
            f64::from(vp.width) / 2.0,
            f64::from(vp.height) / 2.0,
        ));
        view::zoom_at(vp, at, factor);
        self.keep_view = true;
        self.request_redraw();
    }

    /// Draws the board and the overlay and presents them. Does nothing while the window has no
    /// area. Then an export waiting for its frame may run ([`Suspended`]): this frame shows it,
    /// or nothing can.
    fn redraw(&mut self) -> anyhow::Result<()> {
        let Some(gfx) = self.gfx.as_mut() else {
            self.frame_shown();
            return Ok(());
        };
        let size = gfx.window.inner_size();
        let width = u16::try_from(size.width).unwrap_or(u16::MAX);
        let height = u16::try_from(size.height).unwrap_or(u16::MAX);
        let (Some(surface_w), Some(surface_h)) = (
            NonZeroU32::new(u32::from(width)),
            NonZeroU32::new(u32::from(height)),
        ) else {
            self.frame_shown();
            return Ok(());
        };
        if gfx.surface_size != Some((surface_w, surface_h)) {
            gfx.surface
                .resize(surface_w, surface_h)
                .map_err(|err| anyhow!("could not resize the window surface: {err}"))?;
            gfx.surface_size = Some((surface_w, surface_h));
        }

        let scale_factor = gfx.window.scale_factor();
        let vp = frame_view(
            &mut self.viewport,
            &self.engine,
            &mut self.renderer,
            (width, height, scale_factor),
        );

        let render_start = Instant::now();
        let pixels = compose_frame(
            &self.engine,
            &mut self.renderer,
            self.message.as_deref(),
            self.suspended.as_ref(),
            &vp,
        );
        let render_time = render_start.elapsed();

        let mut buffer = gfx
            .surface
            .buffer_mut()
            .map_err(|err| anyhow!("could not get a frame buffer: {err}"))?;
        rgba_to_0rgb(pixels, &mut buffer);
        gfx.window.pre_present_notify();
        buffer
            .present()
            .map_err(|err| anyhow!("could not present a frame: {err}"))?;
        let frame_time = render_start.elapsed();
        self.frame_shown();
        self.frames += 1;
        // Objects over the renderer's work budget are drawn as outlines; it logs how many.
        let drawn = self.renderer.last_frame();
        self.log_frame(
            &drawn,
            render_time,
            frame_time,
            (width, height, scale_factor),
        );
        // Text the frame couldn't afford to lay out is drawn by the next one. Math still
        // compiling asks for a frame itself, through the engine's waker.
        if drawn.incomplete() {
            self.request_redraw();
        }
        Ok(())
    }

    /// Logs a frame just presented, of the window's size in physical pixels at its scale
    /// factor: the first at info level, with the time since lawh started, the others at debug
    /// level.
    fn log_frame(
        &self,
        drawn: &FrameStats,
        render_time: Duration,
        frame_time: Duration,
        (width, height, scale_factor): (u16, u16, f64),
    ) {
        if self.frames == 1 {
            tracing::info!(
                since_start_ms = ms(self.started.elapsed()),
                render_ms = ms(render_time),
                estimated_ms = ms(drawn.estimated),
                width,
                height,
                scale_factor,
                math_pending = drawn.math_pending,
                math_evicted = drawn.math_evicted,
                downgraded = drawn.downgraded,
                text_pending = drawn.text_pending,
                "first frame presented"
            );
        } else {
            tracing::debug!(
                render_ms = ms(render_time),
                estimated_ms = ms(drawn.estimated),
                frame_ms = ms(frame_time),
                width,
                height,
                math_pending = drawn.math_pending,
                math_evicted = drawn.math_evicted,
                downgraded = drawn.downgraded,
                laid_out = drawn.laid_out,
                text_pending = drawn.text_pending,
                "frame"
            );
        }
    }
}

impl Gfx {
    fn new(event_loop: &ActiveEventLoop, title: &str) -> anyhow::Result<Self> {
        let attributes = Window::default_attributes()
            .with_title(title)
            .with_inner_size(INITIAL_SIZE);
        let window = Rc::new(
            event_loop
                .create_window(attributes)
                .context("could not create the window")?,
        );
        // The surface keeps what it needs from the context, so the context can go.
        let context = softbuffer::Context::new(Rc::clone(&window))
            .map_err(|err| anyhow!("could not connect softbuffer to the display: {err}"))?;
        let surface = softbuffer::Surface::new(&context, Rc::clone(&window))
            .map_err(|err| anyhow!("could not create the window surface: {err}"))?;
        Ok(Self {
            window,
            surface,
            surface_size: None,
        })
    }
}

impl ApplicationHandler<MathReady> for App {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, _cause: StartCause) {
        let now = Instant::now();
        // The start of the turn after an export's frame was presented (see `Suspended`).
        if self
            .suspended
            .as_ref()
            .is_some_and(|suspended| suspended.due(now))
        {
            self.run_export();
        }
        if !self.quitting && self.suspended.is_none() && self.timer.fire(now) {
            self.timeout();
        }
        if self.quitting {
            event_loop.exit();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        match Gfx::new(event_loop, &self.title) {
            Ok(gfx) => {
                gfx.window.request_redraw();
                self.gfx = Some(gfx);
            }
            Err(err) => self.fail(event_loop, err),
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, MathReady: MathReady) {
        self.math_ready();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.close_requested(event_loop),
            WindowEvent::Resized(size) => {
                if let Some(vp) = self.viewport.as_mut() {
                    vp.width = u16::try_from(size.width).unwrap_or(u16::MAX);
                    vp.height = u16::try_from(size.height).unwrap_or(u16::MAX);
                }
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(vp) = self.viewport.as_mut() {
                    vp.scale_factor = scale_factor;
                }
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if let Err(err) = self.redraw() {
                    self.fail(event_loop, err);
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => {
                // Synthetic presses are keys held when the window gains focus; repeats count.
                if event.state == ElementState::Pressed && !is_synthetic {
                    self.key(event_loop, &event);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_moved(Point::new(position.x, position.y));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.mouse_input(state, button);
                // A click runs the exports waiting for their frames first, which may quit.
                if self.quitting {
                    event_loop.exit();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.zoom(view::wheel_zoom_factor(delta, self.scale_factor()));
            }
            // A release that happens while another window has focus never arrives here.
            WindowEvent::Focused(false) => {
                self.left = None;
                self.middle_down = false;
                self.modifiers = ModifiersState::empty();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(self.control_flow());
    }
}

/// The status line as the window shows it: [`status::status`] of the engine and the app's
/// `message`, or `exporting "out.png"` while an export waits for its frame (`suspended`).
fn status_line(
    engine: &Engine,
    message: Option<&str>,
    suspended: Option<&Suspended>,
) -> status::Status {
    let mut status = status::status(engine, message);
    if let Some(suspended) = suspended {
        status.left = exporting(&suspended.job.path);
        status.error = false;
    }
    status
}

/// The view a frame of `width` by `height` physical pixels at `scale_factor` is drawn through:
/// the user's (`viewport`), resized to the window, or until there is one, the opening view that
/// fits the board ([`view::opening`]), which then becomes the user's.
fn frame_view(
    viewport: &mut Option<Viewport>,
    engine: &Engine,
    renderer: &mut Renderer,
    (width, height, scale_factor): (u16, u16, f64),
) -> Viewport {
    if let Some(vp) = viewport.as_mut() {
        vp.width = width;
        vp.height = height;
        vp.scale_factor = scale_factor;
        *vp
    } else {
        let bounds = renderer.board_bounds(engine.board());
        let bar = status_bar_height_px(scale_factor);
        let vp = view::opening(bounds, width, height, scale_factor, bar);
        tracing::debug!(?vp, "fitted the board to the window");
        *viewport.insert(vp)
    }
}

/// Draws a frame through `vp` as the window shows it: the board, and over it the engine's
/// cursor, selection, Visual rectangle, caret, command line, jump labels and `:sym` rows, and
/// the status line ([`status_line`] of the engine, the app's `message` and the export waiting
/// for its frame). Premultiplied RGBA8, as [`Renderer::render_with`] returns it.
///
/// [`App::redraw`] presents it; the README demo draws its frames with it too (`app/demo.rs`).
/// A function of the app's fields, not a method, so the window's surface can stay borrowed
/// while the frame is drawn.
fn compose_frame<'r>(
    engine: &Engine,
    renderer: &'r mut Renderer,
    message: Option<&str>,
    suspended: Option<&Suspended>,
    vp: &Viewport,
) -> &'r [u8] {
    let status = status_line(engine, message, suspended);
    let visual = engine.visual_rect(renderer);
    let text_cursor = engine.text_cursor();
    let overlay = Overlay {
        cursor: OverlayCursor::from(engine.cursor()),
        selection: engine.selection(),
        visual,
        caret: text_cursor.as_ref().map(Caret::from),
        status: StatusLine {
            left: &status.left,
            right: &status.right,
            error: status.error,
        },
        command_line: engine.command_line(),
        labels: engine.jump_labels(),
        completions: engine.completions(),
    };
    renderer.render_with(engine.board(), vp, &overlay)
}

/// What the view keeps in sight after a key: the caret in a text block, else the current
/// object, else the cursor's grid point (all in grid units).
fn focus(engine: &Engine, renderer: &mut Renderer) -> Rect {
    let board = engine.board();
    if let Some(caret) = engine.text_cursor()
        && let Some(rect) = renderer.caret_rect(board, &caret.id, caret.byte)
    {
        return rect;
    }
    if let Cursor::Object(id) = engine.cursor()
        && let Some(bounds) = board.get(id).and_then(|o| renderer.object_bounds(o))
    {
        return bounds;
    }
    let p = engine.cursor_point();
    Rect::from_points(p, p)
}

/// Where a click at `at` (physical pixels) lands on the board through `vp`, and how near an
/// object it may land to hit it ([`HIT_TOLERANCE_PX`]), both in grid units. `None` on the
/// status bar.
fn click_point(vp: &Viewport, at: Point) -> Option<(Point, f64)> {
    let bar = f64::from(status_bar_height_px(vp.scale_factor));
    if at.y >= f64::from(vp.height) - bar {
        return None;
    }
    let tolerance = HIT_TOLERANCE_PX * vp.scale_factor / vp.px_per_unit();
    Some((vp.to_board(at), tolerance))
}

/// A close request closes the window when the board has no unwritten changes, or when one was
/// refused already and its warning still shows.
fn close_now(modified: bool, warned: bool) -> bool {
    !modified || warned
}

/// The pointer moved far enough from where the left button went down (physical pixels, at
/// `scale_factor`) for the press to be a drag, not a click.
fn past_click_slop(from: Point, to: Point, scale_factor: f64) -> bool {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let moved = (to - from).hypot();
    // A pointer position that isn't a number is no click.
    moved.is_nan() || moved >= CLICK_SLOP_PX * scale
}

/// Copies premultiplied RGBA8 into softbuffer's `0RGB` pixels. The paper is opaque, so every
/// pixel is too, and premultiplied equals straight.
fn rgba_to_0rgb(rgba: &[u8], out: &mut [u32]) {
    let (pixels, _) = rgba.as_chunks::<4>();
    for (dst, px) in out.iter_mut().zip(pixels) {
        // Little-endian RGBA is 0xAABBGGRR; swapping the bytes and dropping alpha gives 0x00RRGGBB.
        *dst = u32::from_le_bytes(*px).swap_bytes() >> 8;
    }
}

/// What a batch of effects asks for besides writes and exports, once they are done.
#[derive(Debug, Default, PartialEq, Eq)]
struct AfterWrites {
    /// The engine allows quitting ([`Effect::Quit`]).
    quit: bool,
    /// Arm the engine's timer for this many milliseconds ([`Effect::ArmTimer`]).
    timer_ms: Option<u64>,
}

/// The effects of one key or timeout, done in order by [`Effects::run`]: those not done yet,
/// the batch's writes so far, and what else they asked for.
#[derive(Debug, Default)]
struct Effects {
    queue: VecDeque<Effect>,
    /// The board is turned into text once and each file written once for every write of the
    /// batch ([`files::WriteBatch`]), an export between them or not.
    writes: files::WriteBatch,
    /// Filled in as the effects are done; complete once [`Effects::run`] returns `None`.
    after: AfterWrites,
}

impl Effects {
    fn new(effects: Vec<Effect>) -> Self {
        Self {
            queue: effects.into(),
            ..Self::default()
        }
    }

    /// Does the effects in order, up to the next export, which it returns (the effects after
    /// it stay queued for the next run), or to the end or [`Effect::Quit`]: `None`, and
    /// [`Effects::after`] says what else they asked for.
    ///
    /// Writes are reported to `engine` in order with [`Engine::write_done`] (which may add
    /// [`Effect::Quit`] after them). `target` gives the file a write's or an export's path names;
    /// `write` writes the board. Every write is reported, a file written once already in the
    /// batch too, so the engine's writes waiting for their reports all get one.
    ///
    /// A good write is reported as one to the board's own file when `same_file` finds the file it
    /// wrote is the one `:w` alone writes (the target of the engine's file name), however the path
    /// was typed (`~/a.lawh`, an absolute path, a link): the engine then marks the board saved.
    /// Asked after the write, so it sees the files as the write left them. Not for a failed write,
    /// nor while the board has no file name yet (the write that names it is the board's own).
    ///
    /// An export onto the board's own file, by the same comparison or the same path (a board
    /// opened as `notes.svg` and not written yet), is refused at once: reported with
    /// [`export::OWN_FILE`], nothing written, and the effects go on.
    fn run(
        &mut self,
        engine: &mut Engine,
        target: impl Fn(&str) -> PathBuf,
        mut write: impl FnMut(&Path, &[u8]) -> io::Result<()>,
        same_file: impl Fn(&Path, &Path) -> bool,
    ) -> Option<ExportJob> {
        while let Some(effect) = self.queue.pop_front() {
            match effect {
                Effect::Write { path, .. } => {
                    let board = engine.board();
                    let written = target(&path);
                    let result = self
                        .writes
                        .write(written.clone(), || to_lawh(board), &mut write);
                    let own = result.is_ok()
                        && engine
                            .file_name()
                            .is_some_and(|name| same_file(&written, &target(name)));
                    self.queue.extend(engine.write_done(&path, result, own));
                }
                Effect::Export {
                    path,
                    options,
                    selection,
                } => {
                    let written = target(&path);
                    let own = engine.file_name().map(&target);
                    if own.is_some_and(|own| written == own || same_file(&written, &own)) {
                        tracing::info!(path = %written.display(), "an export onto the board's own file refused");
                        engine.export_done(&path, Err(export::OWN_FILE.to_owned()));
                        continue;
                    }
                    return Some(ExportJob {
                        path,
                        target: written,
                        options,
                        selection,
                    });
                }
                Effect::Quit => {
                    self.after.quit = true;
                    self.queue.clear();
                }
                Effect::ArmTimer { ms } => self.after.timer_ms = Some(ms),
            }
        }
        None
    }
}

/// Writes the board's text, `contents`, to `target` for `:w`, logging how it went.
fn write_board(target: &Path, contents: &[u8]) -> io::Result<()> {
    let started = Instant::now();
    let written = files::write_atomic(target, contents);
    match &written {
        Ok(()) => tracing::info!(
            path = %target.display(),
            ms = ms(started.elapsed()),
            "board written"
        ),
        Err(err) => tracing::warn!(path = %target.display(), %err, "could not write the board"),
    }
    written
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod demo;

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;

    use lawh_core::engine::MAX_WRITES_PER_KEY;
    use lawh_core::{FixedMeasure, KeyCode, from_lawh, parse_keys};

    use super::*;
    use crate::test_dir::TestDir;

    #[test]
    fn pixels_become_0rgb() {
        let rgba = [0x11, 0x22, 0x33, 0xFF, 0xFA, 0xFA, 0xF7, 0xFF];
        let mut out = [0xDEAD_BEEF; 2];
        rgba_to_0rgb(&rgba, &mut out);
        assert_eq!(out, [0x0011_2233, 0x00FA_FAF7]);
    }

    #[test]
    fn the_timer_fires_once_after_its_deadline() {
        let now = Instant::now();
        let mut timer = Timer::default();
        assert_eq!(timer.control_flow(), ControlFlow::Wait);
        assert!(!timer.fire(now));

        timer.arm(now, 1000);
        let deadline = now + Duration::from_millis(1000);
        assert_eq!(timer.control_flow(), ControlFlow::WaitUntil(deadline));
        assert!(!timer.fire(now + Duration::from_millis(999)));
        assert!(timer.fire(deadline));
        assert!(!timer.fire(deadline), "only once");
        assert_eq!(timer.control_flow(), ControlFlow::Wait);
    }

    #[test]
    fn a_key_cancels_the_timer_and_a_new_one_replaces_it() {
        let now = Instant::now();
        let mut timer = Timer::default();
        timer.arm(now, 1000);
        timer.cancel();
        assert!(!timer.fire(now + Duration::from_secs(5)));

        timer.arm(now, 1000);
        timer.arm(now + Duration::from_millis(500), 1000);
        assert!(!timer.fire(now + Duration::from_millis(1200)));
        assert!(timer.fire(now + Duration::from_millis(1500)));
    }

    /// An engine for a board named `a.lawh` with one circle, and `Q` mapped to `rhs`.
    fn named_with_q(rhs: &str) -> Engine {
        let board = from_lawh("lawh version=1\ncircle id=c1 x=0 y=0 r=1\n").unwrap();
        let mut engine = Engine::new(board).with_file_name(Some("a.lawh".to_owned()));
        assert_eq!(engine.source(&format!("nnoremap Q {rhs}\n")), []);
        engine
    }

    /// The file a write names, as given.
    fn as_given(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    /// No two paths name one file: only the engine's own comparison of names applies.
    fn different(_: &Path, _: &Path) -> bool {
        false
    }

    /// Does `effects`, which hold no export, as the app does: what else they ask for.
    fn do_writes(
        engine: &mut Engine,
        effects: Vec<Effect>,
        target: impl Fn(&str) -> PathBuf,
        write: impl FnMut(&Path, &[u8]) -> io::Result<()>,
        same_file: impl Fn(&Path, &Path) -> bool,
    ) -> AfterWrites {
        let mut effects = Effects::new(effects);
        assert_eq!(effects.run(engine, target, write, same_file), None);
        assert!(effects.queue.is_empty());
        effects.after
    }

    /// Types `script` (Vim notation): the effects its keys ask for.
    fn type_keys(engine: &mut Engine, script: &str) -> Vec<Effect> {
        let mut effects = Vec::new();
        for key in parse_keys(script).unwrap() {
            effects.extend(engine.feed(key, &mut FixedMeasure));
        }
        effects
    }

    #[test]
    fn a_batch_writes_each_file_once_and_reports_every_write() {
        // Four writes, the most one key may ask for: the board's own file three times, a copy
        // once. Each file is written once, and every write is reported: `:wq` then quits.
        let mut engine = named_with_q(":w<CR>:w b.lawh<CR>:w<CR>:wq<CR>");
        type_keys(&mut engine, ":ci 2<CR>");
        assert!(engine.is_modified());
        let effects = type_keys(&mut engine, "Q");
        assert_eq!(effects.len(), 4, "{effects:?}");
        let mut written = Vec::new();
        let write = |target: &Path, bytes: &[u8]| {
            written.push((target.to_owned(), bytes.to_vec()));
            Ok(())
        };
        let after = do_writes(&mut engine, effects, as_given, write, different);
        let bytes = to_lawh(engine.board()).into_bytes();
        assert_eq!(
            written,
            [
                (PathBuf::from("a.lawh"), bytes.clone()),
                (PathBuf::from("b.lawh"), bytes),
            ]
        );
        assert_eq!(
            after,
            AfterWrites {
                quit: true,
                timer_ms: None
            }
        );
        assert!(!engine.is_modified());
    }

    #[test]
    fn a_failed_write_is_tried_once_and_wq_does_not_quit() {
        let mut engine = named_with_q(":w<CR>:wq<CR>");
        type_keys(&mut engine, ":ci 2<CR>");
        let effects = type_keys(&mut engine, "Q");
        let mut tries = 0;
        let write = |_: &Path, _: &[u8]| {
            tries += 1;
            Err(io::Error::other("disk full"))
        };
        let after = do_writes(&mut engine, effects, as_given, write, different);
        assert_eq!(tries, 1);
        assert_eq!(after, AfterWrites::default());
        assert!(engine.is_modified());
        let message = engine.message().unwrap();
        assert!(message.error);
        assert_eq!(message.text, "cannot write \"a.lawh\": disk full");
    }

    #[test]
    fn the_status_line_keeps_a_keys_error_over_its_later_write_reports() {
        // A copy that fails, then the board's own write that goes well: the failure stays.
        let copy = "/no/such/dir/b.lawh";
        let mut engine = named_with_q(&format!(":w {copy}<CR>:w<CR>"));
        type_keys(&mut engine, ":ci 2<CR>");
        let effects = type_keys(&mut engine, "Q");
        let write = |target: &Path, _: &[u8]| {
            if target == Path::new(copy) {
                Err(io::Error::other("no such directory"))
            } else {
                Ok(())
            }
        };
        let after = do_writes(&mut engine, effects, as_given, write, different);
        assert_eq!(after, AfterWrites::default());
        assert!(!engine.is_modified());
        let shown = status::status(&engine, None);
        assert!(shown.error, "{shown:?}");
        assert_eq!(
            shown.left,
            format!("cannot write \"{copy}\": no such directory")
        );

        // Four good writes, then a `:wq` refused as one too many: lawh stays open and says why.
        let mut engine = named_with_q(":w<CR>:w<CR>:w<CR>:w<CR>:wq<CR>");
        type_keys(&mut engine, ":ci 2<CR>");
        let effects = type_keys(&mut engine, "Q");
        let after = do_writes(&mut engine, effects, as_given, |_, _| Ok(()), different);
        assert_eq!(after, AfterWrites::default());
        assert!(!engine.is_modified());
        let shown = status::status(&engine, None);
        assert!(shown.error, "{shown:?}");
        assert_eq!(
            shown.left,
            format!(
                "that key asked for more than {MAX_WRITES_PER_KEY} writes; the rest of it was \
                 dropped"
            )
        );
    }

    #[test]
    fn a_good_write_is_compared_with_the_boards_own_file() {
        // `same` takes `alias.lawh` for the board's file `a.lawh`, which the engine can't tell
        // from the names. It is asked after each good write, not after the failed one.
        let mut engine = named_with_q(":w b.lawh<CR>:w alias.lawh<CR>:w bad.lawh<CR>");
        type_keys(&mut engine, ":ci 2<CR>");
        let effects = type_keys(&mut engine, "Q");
        let asked = RefCell::new(Vec::new());
        let write = |target: &Path, _: &[u8]| {
            if target == Path::new("bad.lawh") {
                Err(io::Error::other("disk full"))
            } else {
                Ok(())
            }
        };
        let same = |a: &Path, b: &Path| {
            asked.borrow_mut().push((a.to_owned(), b.to_owned()));
            a == Path::new("alias.lawh") && b == Path::new("a.lawh")
        };
        do_writes(&mut engine, effects, as_given, write, same);
        let pair = |a: &str, b: &str| (PathBuf::from(a), PathBuf::from(b));
        assert_eq!(
            asked.take(),
            [pair("b.lawh", "a.lawh"), pair("alias.lawh", "a.lawh")]
        );
        assert!(!engine.is_modified());

        // An unnamed board: the write that names it is to its own file without asking.
        let board = from_lawh("lawh version=1\ncircle id=c1 x=0 y=0 r=1\n").unwrap();
        let mut engine = Engine::new(board);
        assert_eq!(
            engine.source("nnoremap Q :w a.lawh<CR>:w alias.lawh<CR>\n"),
            []
        );
        type_keys(&mut engine, ":ci 2<CR>");
        let effects = type_keys(&mut engine, "Q");
        do_writes(&mut engine, effects, as_given, |_, _| Ok(()), same);
        assert_eq!(asked.take(), [pair("alias.lawh", "a.lawh")]);
        assert_eq!(engine.file_name(), Some("a.lawh"));
        assert!(!engine.is_modified());
    }

    #[test]
    fn closing_asks_once_when_there_are_changes() {
        assert!(close_now(false, false));
        assert!(!close_now(true, false));
        assert!(close_now(true, true));
    }

    #[test]
    fn a_click_is_a_press_that_barely_moves() {
        let at = Point::new(100.0, 100.0);
        assert!(!past_click_slop(at, at, 1.0));
        assert!(!past_click_slop(at, Point::new(103.0, 102.0), 1.0));
        assert!(past_click_slop(at, Point::new(104.0, 100.0), 1.0));
        // At scale factor 2 the slop is twice as many physical pixels.
        assert!(!past_click_slop(at, Point::new(107.0, 100.0), 2.0));
        assert!(past_click_slop(at, Point::new(108.0, 100.0), 2.0));
        assert!(past_click_slop(at, Point::new(f64::NAN, 100.0), 1.0));
        assert!(!past_click_slop(at, Point::new(103.0, 100.0), f64::NAN));
    }

    #[test]
    fn a_click_selects_what_is_under_it_or_the_nearest_grid_point() {
        let mut renderer = Renderer::new_trusted();
        let board = from_lawh("lawh version=1\nline id=l1 x1=0 y1=0 x2=4 y2=0\n").unwrap();
        let mut engine = Engine::new(board);
        // 40 px a unit, the origin at the window's centre (400, 300).
        let vp = Viewport::new(Point::ZERO, 1.0, 800, 600, 1.0);
        let click = |engine: &mut Engine, renderer: &mut Renderer, at: Point| {
            let (point, tolerance) = click_point(&vp, at).unwrap();
            let hit = renderer.hit_test(engine.board(), point, tolerance);
            engine.click(point, hit, renderer);
        };

        // 3 px below the line: on it.
        click(&mut engine, &mut renderer, Point::new(480.0, 303.0));
        assert_eq!(
            engine.cursor(),
            &Cursor::Object(ObjectId::new("l1").unwrap())
        );
        // 5 px below: past the tolerance, so the nearest grid point.
        click(&mut engine, &mut renderer, Point::new(480.0, 305.0));
        assert_eq!(engine.cursor(), &Cursor::Grid(Point::new(2.0, 0.0)));
        click(&mut engine, &mut renderer, Point::new(530.0, 250.0));
        assert_eq!(engine.cursor(), &Cursor::Grid(Point::new(3.0, -1.0)));
    }

    #[test]
    fn the_click_tolerance_is_logical_pixels_and_the_status_bar_is_not_the_board() {
        let vp = Viewport::new(Point::new(5.0, 5.0), 2.0, 800, 600, 2.0);
        let (point, tolerance) = click_point(&vp, Point::new(400.0, 300.0)).unwrap();
        assert_eq!(point, Point::new(5.0, 5.0));
        // 4 logical px at zoom 2: 4 / (40 x 2) units.
        assert!((tolerance - 0.05).abs() < 1e-12, "{tolerance}");
        // The bar is 48 physical px at scale factor 2.
        assert!(click_point(&vp, Point::new(10.0, 551.0)).is_some());
        assert!(click_point(&vp, Point::new(10.0, 552.0)).is_none());
    }

    fn feed(engine: &mut Engine, keys: &str) {
        for key in parse_keys(keys).unwrap() {
            engine.feed(key, &mut FixedMeasure);
        }
    }

    #[test]
    fn the_view_follows_the_caret_then_the_object_then_the_point() {
        let mut renderer = Renderer::new_trusted();
        let board = from_lawh("lawh version=1\nrect id=r1 x=10 y=20 w=4 h=2\n").unwrap();
        let mut engine = Engine::new(board);
        assert_eq!(focus(&engine, &mut renderer), Rect::new(0.0, 0.0, 0.0, 0.0));
        feed(&mut engine, "3l");
        assert_eq!(focus(&engine, &mut renderer), Rect::new(3.0, 0.0, 3.0, 0.0));
        feed(&mut engine, "w");
        assert_eq!(
            focus(&engine, &mut renderer),
            Rect::new(10.0, 20.0, 14.0, 22.0)
        );
        // A new text block below the rectangle: the caret, zero wide, one line high.
        feed(&mut engine, "o");
        assert_eq!(engine.mode(), Mode::Insert);
        let caret = focus(&engine, &mut renderer);
        assert!(caret.width().abs() < 1e-9, "{caret:?}");
        assert!(caret.height() > 0.0, "{caret:?}");
        assert!(caret.y0 > 22.0, "{caret:?}");
    }

    fn app_for(board: lawh_core::Board) -> App {
        app_with(Engine::new(board), None)
    }

    fn app_with(engine: Engine, message: Option<String>) -> App {
        App::new(
            engine,
            None,
            None,
            Renderer::new_trusted(),
            Instant::now(),
            message,
        )
    }

    #[test]
    fn a_close_after_a_click_or_another_message_warns_again() {
        let modified = |message: Option<&str>| {
            let mut engine = Engine::new(lawh_core::Board::new());
            feed(&mut engine, ":ci 1<CR>");
            assert!(engine.is_modified());
            let mut app = app_with(engine, message.map(str::to_owned));
            // 40 px a unit, the origin at the window's centre (400, 300).
            app.viewport = Some(Viewport::new(Point::ZERO, 1.0, 800, 600, 1.0));
            app
        };
        // Two closes in a row: the first warns, the second closes.
        let mut app = modified(None);
        assert!(!app.close_or_warn());
        assert_eq!(app.message.as_deref(), Some(CLOSE_MODIFIED));
        assert!(app.close_or_warn());
        // A click on the board drops the warning, and the next close warns again.
        let mut app = modified(None);
        assert!(!app.close_or_warn());
        app.click(Point::new(480.0, 300.0));
        assert_eq!(app.message, None);
        assert!(!app.close_or_warn());
        assert!(app.close_or_warn());
        // A click on the status bar does nothing: the warning stays, and so does the close.
        let mut app = modified(None);
        assert!(!app.close_or_warn());
        app.click(Point::new(10.0, 599.0));
        assert_eq!(app.message.as_deref(), Some(CLOSE_MODIFIED));
        assert!(app.close_or_warn());
        // Another message of the app's (a lawhrc warning) is no close warning.
        let mut app = modified(Some("lawhrc line 2: nope"));
        assert!(!app.close_or_warn());
        assert!(app.close_or_warn());
        // Without changes, a close closes at once.
        assert!(app_for(lawh_core::Board::new()).close_or_warn());
    }

    #[test]
    fn the_cursor_starts_on_the_first_object_and_the_first_key_does_not_pan() {
        let board = from_lawh(
            "lawh version=1\n\
             rect id=r1 x=30 y=8 w=4 h=2\n\
             circle id=c1 x=40 y=-3 r=1\n\
             text id=t1 x=-20 y=-10 \"first\"\n\
             line id=l1 x1=0 y1=12 x2=50 y2=12\n",
        )
        .unwrap();
        let mut app = app_for(board);
        assert_eq!(
            app.engine.cursor(),
            &Cursor::Object(ObjectId::new("t1").unwrap())
        );
        for (width, height, scale) in [(1280, 800, 1.0), (800, 1200, 2.0), (2560, 600, 1.5)] {
            let bar = status_bar_height_px(scale);
            let bounds = app.renderer.board_bounds(app.engine.board());
            let vp = view::opening(bounds, width, height, scale, bar);
            // What `after_input` does after the first key that leaves the cursor where it is.
            let mut followed = vp;
            let focus = focus(&app.engine, &mut app.renderer);
            view::follow(&mut followed, focus, f64::from(bar));
            assert_eq!(followed, vp, "{width}x{height} at {scale}");
        }

        // An empty board keeps the grid's origin.
        let app = app_for(lawh_core::Board::new());
        assert_eq!(app.engine.cursor(), &Cursor::Grid(Point::ZERO));
    }

    fn keys(script: &str) -> Vec<Key> {
        parse_keys(script).unwrap()
    }

    /// The ids of the objects with jump labels, in order of id.
    fn labelled(app: &App) -> Vec<String> {
        let (labels, _) = app.engine.jump_labels().unwrap();
        let mut ids: Vec<_> = labels.iter().map(|l| l.id.as_str().to_owned()).collect();
        ids.sort();
        ids
    }

    /// Five small circles. The cursor starts on `a`. In an 800x600 window on the origin at zoom
    /// 1 and scale factor 1, x from -10 to 10 units and y from -7.5 to 7.5: `b` and `c` show,
    /// `d` is behind the status bar (its 24 px are the 0.6 units at the bottom) and `e` is
    /// right of the window.
    const SPREAD: &str = "lawh version=1\n\
                          circle id=a x=0 y=-3 r=0.1\n\
                          circle id=b x=5 y=0 r=0.1\n\
                          circle id=c x=0 y=6.5 r=0.1\n\
                          circle id=d x=0 y=7.2 r=0.1\n\
                          circle id=e x=30 y=0 r=0.1\n";

    fn origin_view() -> Viewport {
        Viewport::new(Point::ZERO, 1.0, 800, 600, 1.0)
    }

    #[test]
    fn s_labels_what_the_window_shows_above_the_status_bar_when_it_runs() {
        let mut app = app_for(from_lawh(SPREAD).unwrap());
        assert_eq!(
            app.engine.cursor(),
            &Cursor::Object(ObjectId::new("a").unwrap())
        );
        // Before the first frame there is no view: that frame will show the whole board.
        app.feed(keys("s"));
        assert_eq!(labelled(&app), ["b", "c", "d", "e"]);
        app.feed(keys("<Esc>"));
        assert!(app.engine.jump_labels().is_none());

        app.viewport = Some(origin_view());
        app.feed(keys("s"));
        assert_eq!(labelled(&app), ["b", "c"]);
        // Panned since: the next `s` labels what shows then.
        app.viewport.as_mut().unwrap().center = Point::new(30.0, 0.0);
        app.feed(keys("<Esc>s"));
        assert_eq!(labelled(&app), ["e"]);
        // Zoomed out, the window shows them all.
        app.feed(keys("<Esc>"));
        view::zoom_at(
            app.viewport.as_mut().unwrap(),
            Point::new(400.0, 300.0),
            0.1,
        );
        app.feed(keys("s"));
        assert_eq!(labelled(&app), ["b", "c", "d", "e"]);
        // A resize: only the top half of the window at zoom 1 on the origin.
        app.feed(keys("<Esc>"));
        app.viewport = Some(Viewport::new(Point::ZERO, 1.0, 800, 300, 1.0));
        app.feed(keys("s"));
        assert_eq!(labelled(&app), ["b"]);
        // A label typed: the cursor goes there.
        let (labels, _) = app.engine.jump_labels().unwrap();
        let label = labels[0].text.clone();
        app.feed(keys(&label));
        assert!(app.engine.jump_labels().is_none());
        assert_eq!(
            app.engine.cursor(),
            &Cursor::Object(ObjectId::new("b").unwrap())
        );
    }

    #[test]
    fn s_run_by_the_mapping_timeout_labels_the_view_as_it_is_then() {
        let mut engine = Engine::new(from_lawh(SPREAD).unwrap());
        assert_eq!(engine.source("nnoremap sz l\n"), []);
        let mut app = app_with(engine, None);
        app.viewport = Some(origin_view());
        app.feed(keys("s"));
        assert!(app.engine.jump_labels().is_none(), "waiting for `sz`");
        assert!(app.timer.deadline.is_some());
        app.viewport.as_mut().unwrap().center = Point::new(30.0, 0.0);
        app.timeout();
        assert_eq!(labelled(&app), ["e"]);
    }

    /// An app for the board in `dir`'s file `name` (an empty board if there is none), opened as
    /// `name` from `dir`, which is also the home directory.
    fn opened_app_in(dir: &Path, name: &str) -> App {
        let board = fs::read_to_string(dir.join(name))
            .map_or_else(|_| lawh_core::Board::new(), |src| from_lawh(&src).unwrap());
        let engine = Engine::new(board).with_file_name(Some(name.to_owned()));
        let mut app = App::new(
            engine,
            Some(PathBuf::from(name)),
            Some(dir.to_owned()),
            Renderer::new_trusted(),
            Instant::now(),
            None,
        );
        app.home = Some(dir.to_owned());
        app
    }

    /// [`opened_app_in`], with a change not written yet.
    fn changed_app_in(dir: &Path, name: &str) -> App {
        let mut app = opened_app_in(dir, name);
        app.feed(keys(":ci 2<CR>"));
        assert!(app.engine.is_modified());
        app
    }

    /// Types `:w path` and Enter, the path's characters as typed (a `<` or `\` in it is no key
    /// notation).
    fn write_as(app: &mut App, path: &str) {
        let mut keys: Vec<Key> = ":w ".chars().chain(path.chars()).map(Key::char).collect();
        keys.push(Key::named(KeyCode::Enter));
        app.feed(keys);
    }

    const ONE_CIRCLE: &str = "lawh version=1\ncircle id=c1 x=0 y=0 r=1\n";

    #[test]
    fn a_write_to_the_boards_own_file_by_another_path_marks_it_saved() {
        let dir = TestDir::new("own-file");
        let x = dir.path("x.lawh");
        let absolute = x.to_str().unwrap().to_owned();
        for path in ["~/x.lawh", absolute.as_str(), "./x.lawh"] {
            dir.write("x.lawh", ONE_CIRCLE);
            let mut app = changed_app_in(dir.root(), "x.lawh");
            write_as(&mut app, path);
            assert!(!app.engine.is_modified(), "{path}");
            let message = app.engine.message().unwrap();
            assert_eq!(message.text, format!("\"{path}\" written"));
            assert_eq!(
                fs::read_to_string(&x).unwrap(),
                to_lawh(app.engine.board()),
                "{path}"
            );
        }
        // Another file is a copy: the board keeps its changes, and its file the old board.
        dir.write("x.lawh", ONE_CIRCLE);
        let mut app = changed_app_in(dir.root(), "x.lawh");
        write_as(&mut app, "~/y.lawh");
        assert!(app.engine.is_modified());
        let copy = fs::read_to_string(dir.path("y.lawh")).unwrap();
        assert_eq!(copy, to_lawh(app.engine.board()));
        assert_eq!(fs::read_to_string(&x).unwrap(), ONE_CIRCLE);
    }

    #[cfg(unix)]
    #[test]
    fn a_write_through_a_link_to_the_boards_file_marks_it_saved() {
        let dir = TestDir::new("own-file-link");
        let x = dir.write("x.lawh", ONE_CIRCLE);
        std::os::unix::fs::symlink("x.lawh", dir.path("link.lawh")).unwrap();
        let mut app = changed_app_in(dir.root(), "x.lawh");
        write_as(&mut app, "link.lawh");
        assert!(!app.engine.is_modified());
        assert_eq!(fs::read_to_string(&x).unwrap(), to_lawh(app.engine.board()));

        // A board opened through the link, written by the file's own name.
        let mut app = changed_app_in(dir.root(), "link.lawh");
        write_as(&mut app, "x.lawh");
        assert!(!app.engine.is_modified());
        assert_eq!(fs::read_to_string(&x).unwrap(), to_lawh(app.engine.board()));

        // A hard link is replaced by the write, so the board's file keeps the old board.
        fs::hard_link(&x, dir.path("hard.lawh")).unwrap();
        let old = fs::read_to_string(&x).unwrap();
        let mut app = changed_app_in(dir.root(), "x.lawh");
        write_as(&mut app, "hard.lawh");
        assert!(app.engine.is_modified());
        assert_eq!(fs::read_to_string(&x).unwrap(), old);
    }

    const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

    /// Types `:exp path` and Enter, the path's characters as typed.
    fn export_as(app: &mut App, path: &str) {
        let mut keys: Vec<Key> = ":exp ".chars().chain(path.chars()).map(Key::char).collect();
        keys.push(Key::named(KeyCode::Enter));
        app.feed(keys);
    }

    /// The status line the next frame shows.
    fn shown(app: &App) -> status::Status {
        status_line(&app.engine, app.message.as_deref(), app.suspended.as_ref())
    }

    /// The export waiting for its frame.
    fn waiting(app: &App) -> &Suspended {
        app.suspended
            .as_ref()
            .expect("an export waits for its frame")
    }

    #[test]
    fn an_export_shows_that_it_starts_then_writes_and_reports() {
        let dir = TestDir::new("export-frame");
        dir.write("a.lawh", ONE_CIRCLE);
        let mut app = opened_app_in(dir.root(), "a.lawh");
        export_as(&mut app, "out.png");
        // The frame comes first: nothing is written yet, and the status line says why.
        let out = dir.path("out.png");
        assert!(!out.exists());
        let status = shown(&app);
        assert_eq!(status.left, "exporting \"out.png\"");
        assert!(!status.error);
        let suspended = waiting(&app);
        assert_eq!(suspended.job.target, out);
        assert_eq!(suspended.job.selection, None);
        assert_eq!(
            app.control_flow(),
            ControlFlow::WaitUntil(suspended.deadline)
        );
        // Once a frame is presented, the event loop doesn't wait: the next turn runs it.
        app.frame_shown();
        assert_eq!(app.control_flow(), ControlFlow::Poll);
        app.run_export();
        assert!(app.suspended.is_none());
        assert!(fs::read(&out).unwrap().starts_with(PNG_SIGNATURE));
        let status = shown(&app);
        assert_eq!(status.left, "\"out.png\" exported");
        assert!(!status.error);
        assert_eq!(app.control_flow(), ControlFlow::Wait);
        // The board is neither written nor renamed.
        assert_eq!(fs::read_to_string(dir.path("a.lawh")).unwrap(), ONE_CIRCLE);
        assert_eq!(app.engine.file_name(), Some("a.lawh"));
        assert!(!app.engine.is_modified());
    }

    #[test]
    fn an_export_waits_for_its_frame_only_so_long() {
        let dir = TestDir::new("export-deadline");
        dir.write("a.lawh", ONE_CIRCLE);
        let mut app = opened_app_in(dir.root(), "a.lawh");
        export_as(&mut app, "out.svg");
        let deadline = waiting(&app).deadline;
        let typed = Instant::now();
        assert!(deadline > typed && deadline <= typed + EXPORT_FRAME_WAIT);
        let suspended = waiting(&app);
        let just_before = deadline.checked_sub(Duration::from_millis(1)).unwrap();
        assert!(!suspended.due(just_before));
        assert!(suspended.due(deadline));
        // A window with nothing to draw on (none yet, here) doesn't hold it up.
        app.redraw().unwrap();
        assert!(waiting(&app).shown);
        assert!(waiting(&app).due(typed));
    }

    #[test]
    fn exports_find_their_file_as_writes_do() {
        let dir = TestDir::new("export-paths");
        dir.write("a.lawh", ONE_CIRCLE);
        fs::create_dir(dir.path("sub")).unwrap();
        let absolute = dir.path("abs.PDF");
        let absolute_name = absolute.to_str().unwrap().to_owned();
        let cases = [
            ("out.svg", dir.path("out.svg"), &b"<?xml"[..]),
            ("~/sub/out.pdf", dir.path("sub").join("out.pdf"), b"%PDF-"),
            ("sub/../up.png", dir.path("sub/../up.png"), PNG_SIGNATURE),
            (absolute_name.as_str(), absolute.clone(), b"%PDF-"),
        ];
        for (typed, file, signature) in cases {
            let mut app = opened_app_in(dir.root(), "a.lawh");
            export_as(&mut app, typed);
            assert_eq!(waiting(&app).job.target, file, "{typed}");
            app.run_export();
            assert!(fs::read(&file).unwrap().starts_with(signature), "{typed}");
            assert_eq!(
                app.engine.message().unwrap().text,
                format!("\"{typed}\" exported")
            );
        }
        // Again: the file is replaced.
        let mut app = opened_app_in(dir.root(), "a.lawh");
        fs::write(dir.path("out.svg"), "old").unwrap();
        export_as(&mut app, "out.svg");
        app.run_export();
        assert!(fs::read(dir.path("out.svg")).unwrap().starts_with(b"<?xml"));
    }

    #[test]
    fn a_visual_selection_is_what_is_exported() {
        let dir = TestDir::new("export-visual");
        dir.write("a.lawh", ONE_CIRCLE);
        let mut app = opened_app_in(dir.root(), "a.lawh");
        app.feed(keys("v:exp sel.svg<CR>"));
        assert_eq!(
            waiting(&app).job.selection,
            Some(vec![ObjectId::new("c1").unwrap()])
        );
        app.run_export();
        assert!(dir.path("sel.svg").exists());
    }

    #[test]
    fn an_export_onto_the_boards_own_file_is_refused() {
        let dir = TestDir::new("export-own-file");
        let own = dir.write("x.svg", ONE_CIRCLE);
        #[cfg(unix)]
        std::os::unix::fs::symlink("x.svg", dir.path("link.png")).unwrap();
        let mut typed = vec!["x.svg", "./x.svg", "~/x.svg", "sub/../x.svg"];
        let absolute = own.to_str().unwrap().to_owned();
        typed.push(&absolute);
        if cfg!(unix) {
            typed.push("link.png");
        }
        fs::create_dir(dir.path("sub")).unwrap();
        for path in typed {
            let mut app = opened_app_in(dir.root(), "x.svg");
            export_as(&mut app, path);
            assert!(app.suspended.is_none(), "{path}: refused at once");
            let status = shown(&app);
            assert!(status.error, "{path}");
            assert_eq!(
                status.left,
                format!("cannot export \"{path}\": that is the board's own file")
            );
            assert_eq!(fs::read_to_string(&own).unwrap(), ONE_CIRCLE, "{path}");
        }
        // A board whose file isn't written yet: the same name is the same file.
        let mut app = opened_app_in(dir.root(), "new.pdf");
        export_as(&mut app, "new.pdf");
        assert!(app.suspended.is_none());
        assert!(!dir.path("new.pdf").exists());
        // Another file is no copy of the board: it is exported.
        let mut app = opened_app_in(dir.root(), "x.svg");
        export_as(&mut app, "y.svg");
        app.run_export();
        assert!(dir.path("y.svg").exists());
        assert_eq!(fs::read_to_string(&own).unwrap(), ONE_CIRCLE);
    }

    #[test]
    fn effects_and_keys_after_an_export_wait_for_it_in_order() {
        let dir = TestDir::new("export-order");
        dir.write("a.lawh", ONE_CIRCLE);
        let mut app = opened_app_in(dir.root(), "a.lawh");
        assert_eq!(
            app.engine
                .source("nnoremap Q :w b.lawh<CR>:exp out.svg<CR>:w c.lawh<CR>\n"),
            []
        );
        let before = to_lawh(app.engine.board());
        // `x` after `Q` in one batch of keys, `j` typed while the export waits.
        app.feed(keys("Qx"));
        app.feed(keys("j"));
        assert_eq!(fs::read_to_string(dir.path("b.lawh")).unwrap(), before);
        assert!(!dir.path("out.svg").exists());
        assert!(!dir.path("c.lawh").exists());
        assert_eq!(waiting(&app).keys, keys("xj"));
        assert_eq!(app.engine.board().len(), 1, "`x` waits for the export");

        app.run_export();
        // The export drew the board before `x` deleted its circle, and the write after it
        // wrote that board too.
        assert!(fs::read(dir.path("out.svg")).unwrap().starts_with(b"<?xml"));
        assert_eq!(fs::read_to_string(dir.path("c.lawh")).unwrap(), before);
        assert!(app.engine.board().is_empty());
        assert!(app.suspended.is_none());
    }

    #[test]
    fn a_quit_after_an_export_waits_for_it() {
        let dir = TestDir::new("export-quit");
        dir.write("a.lawh", ONE_CIRCLE);
        let mut app = opened_app_in(dir.root(), "a.lawh");
        assert_eq!(
            app.engine.source("nnoremap Q :exp out.pdf<CR>:wq<CR>\n"),
            []
        );
        app.feed(keys("Q"));
        assert!(!app.quitting);
        assert!(app.suspended.is_some());
        app.run_export();
        assert!(fs::read(dir.path("out.pdf")).unwrap().starts_with(b"%PDF-"));
        assert!(app.quitting);
    }

    #[test]
    fn a_failed_export_says_why_and_writes_nothing() {
        let dir = TestDir::new("export-failed");
        dir.write("a.lawh", ONE_CIRCLE);
        let mut app = opened_app_in(dir.root(), "a.lawh");
        export_as(&mut app, "no-such-dir/out.png");
        app.run_export();
        let status = shown(&app);
        assert!(status.error);
        assert!(
            status
                .left
                .starts_with("cannot export \"no-such-dir/out.png\": "),
            "{}",
            status.left
        );
        assert!(!dir.path("no-such-dir").exists());
    }

    #[test]
    fn a_click_runs_a_waiting_export_and_its_keys_first() {
        let dir = TestDir::new("export-click");
        dir.write("a.lawh", ONE_CIRCLE);
        let mut app = opened_app_in(dir.root(), "a.lawh");
        app.viewport = Some(origin_view());
        export_as(&mut app, "out.png");
        app.feed(keys("x"));
        // On the circle's centre: the circle is gone by then, so a grid point.
        app.click(Point::new(400.0, 300.0));
        assert!(
            fs::read(dir.path("out.png"))
                .unwrap()
                .starts_with(PNG_SIGNATURE)
        );
        assert!(app.engine.board().is_empty());
        assert_eq!(app.engine.cursor(), &Cursor::Grid(Point::ZERO));
    }

    /// Two circles: `c1` on the origin, where the cursor starts, and `c2` 5 units right of it,
    /// at (600, 300) in [`origin_view`].
    const TWO_CIRCLES: &str = "lawh version=1\n\
                               circle id=c1 x=0 y=0 r=1\n\
                               circle id=c2 x=5 y=0 r=1\n";

    fn object(name: &str) -> Cursor {
        Cursor::Object(ObjectId::new(name).unwrap())
    }

    #[test]
    fn a_click_runs_every_waiting_export_and_their_keys_first() {
        let dir = TestDir::new("export-click-all");
        dir.write("a.lawh", TWO_CIRCLES);
        let mut app = opened_app_in(dir.root(), "a.lawh");
        app.viewport = Some(origin_view());
        assert_eq!(app.engine.cursor(), &object("c1"));
        // Typed fast: the second export and `x` wait behind the first.
        app.feed(keys(":exp one.png<CR>:exp two.png<CR>x"));
        assert_eq!(waiting(&app).keys, keys(":exp two.png<CR>x"));
        // A click on `c2`: both exports run, and `x` deletes `c1`, before the click moves the
        // cursor.
        app.click(Point::new(600.0, 300.0));
        assert!(app.suspended.is_none());
        let one = fs::read(dir.path("one.png")).unwrap();
        assert!(one.starts_with(PNG_SIGNATURE));
        assert_eq!(
            fs::read(dir.path("two.png")).unwrap(),
            one,
            "both drew the board before `x`"
        );
        let ids: Vec<_> = app
            .engine
            .board()
            .objects()
            .iter()
            .map(|o| o.id.as_str())
            .collect();
        assert_eq!(ids, ["c2"]);
        assert_eq!(app.engine.cursor(), &object("c2"));
    }

    /// `c1` on the origin, where the cursor starts, and `c2` 100 units right of it, far outside
    /// [`origin_view`].
    const FAR_APART: &str = "lawh version=1\n\
                             circle id=c1 x=0 y=0 r=1\n\
                             circle id=c2 x=100 y=0 r=1\n";

    #[test]
    fn a_click_lands_through_the_view_the_user_clicked_on() {
        let dir = TestDir::new("export-click-view");
        dir.write("a.lawh", FAR_APART);
        // The keys that wait move the view: `w` goes to `c2`, and the view follows it.
        let mut app = opened_app_in(dir.root(), "a.lawh");
        app.viewport = Some(origin_view());
        app.feed(keys(":exp one.png<CR>w"));
        assert!(app.finish_exports());
        assert_eq!(app.engine.cursor(), &object("c2"));
        assert_ne!(app.viewport, Some(origin_view()));

        // Typed fast, then a click on `c1` where the window shows it, before any frame: the
        // export and `w` run first, then the click lands on `c1`, through the view the user
        // saw, which stays.
        let mut app = opened_app_in(dir.root(), "a.lawh");
        app.viewport = Some(origin_view());
        app.feed(keys(":exp one.png<CR>w"));
        assert!(app.suspended.is_some());
        app.click(Point::new(400.0, 300.0));
        assert!(app.suspended.is_none());
        assert!(
            fs::read(dir.path("one.png"))
                .unwrap()
                .starts_with(PNG_SIGNATURE)
        );
        assert_eq!(app.engine.cursor(), &object("c1"));
        assert_eq!(app.viewport, Some(origin_view()));
        // On the grid there too: 2 units right of `c1`'s centre is (2, 0).
        let mut app = opened_app_in(dir.root(), "a.lawh");
        app.viewport = Some(origin_view());
        app.feed(keys(":exp one.png<CR>w"));
        app.click(Point::new(480.0, 300.0));
        assert_eq!(app.engine.cursor(), &Cursor::Grid(Point::new(2.0, 0.0)));
    }

    #[test]
    fn a_click_keeps_what_the_exports_it_ran_reported() {
        let dir = TestDir::new("export-click-report");
        dir.write("a.lawh", TWO_CIRCLES);
        // A failed export, run by the click: its error stays after it.
        let mut app = opened_app_in(dir.root(), "a.lawh");
        app.viewport = Some(origin_view());
        export_as(&mut app, "no-such-dir/out.png");
        app.click(Point::new(600.0, 300.0));
        assert_eq!(app.engine.cursor(), &object("c2"));
        let status = shown(&app);
        assert!(status.error);
        assert!(
            status
                .left
                .starts_with("cannot export \"no-such-dir/out.png\": "),
            "{}",
            status.left
        );
        // Until the next key, as any message.
        app.feed(keys("<Esc>"));
        assert_eq!(shown(&app).left, "");

        // A good export's report stays too.
        let mut app = opened_app_in(dir.root(), "a.lawh");
        app.viewport = Some(origin_view());
        export_as(&mut app, "out.svg");
        app.click(Point::new(600.0, 300.0));
        let status = shown(&app);
        assert_eq!(status.left, "\"out.svg\" exported");
        assert!(!status.error);
        // A click with no export waiting drops the message, which a frame has shown.
        app.click(Point::new(400.0, 300.0));
        assert_eq!(app.engine.message(), None);
        assert_eq!(app.engine.cursor(), &object("c1"));

        // What the keys after the export left, as without the click: here `:w` reports.
        let mut app = changed_app_in(dir.root(), "a.lawh");
        app.viewport = Some(origin_view());
        app.feed(keys(":exp two.svg<CR>:w<CR>"));
        app.click(Point::new(600.0, 300.0));
        assert_eq!(shown(&app).left, "\"a.lawh\" written");
        assert!(!app.engine.is_modified());
    }

    #[test]
    fn a_close_shows_an_export_it_ran_failing_before_its_warning() {
        let dir = TestDir::new("export-close-report");
        dir.write("a.lawh", TWO_CIRCLES);
        let failed = |app: &App| {
            let status = shown(app);
            status.error
                && status
                    .left
                    .starts_with("cannot export \"no-such-dir/out.png\": ")
        };
        // A changed board: the export's error first, then the close warning, then the close.
        let mut app = changed_app_in(dir.root(), "a.lawh");
        export_as(&mut app, "no-such-dir/out.png");
        assert!(!app.close());
        assert!(failed(&app), "{:?}", shown(&app));
        assert_eq!(app.message, None);
        assert!(!app.close());
        assert_eq!(shown(&app).left, CLOSE_MODIFIED);
        assert!(app.close());

        // No changes: the error, then the close.
        let mut app = opened_app_in(dir.root(), "a.lawh");
        export_as(&mut app, "no-such-dir/out.png");
        assert!(!app.close());
        assert!(failed(&app), "{:?}", shown(&app));
        assert!(app.close());

        // An error the user saw already doesn't hold the close.
        let mut app = opened_app_in(dir.root(), "a.lawh");
        export_as(&mut app, "no-such-dir/out.png");
        app.run_export();
        assert!(failed(&app));
        assert!(app.close());

        // A good export: the window closes at once without changes, and warns with them.
        let mut app = opened_app_in(dir.root(), "a.lawh");
        export_as(&mut app, "good.png");
        assert!(app.close());
        let mut app = changed_app_in(dir.root(), "a.lawh");
        export_as(&mut app, "good.png");
        assert!(!app.close());
        assert_eq!(shown(&app).left, CLOSE_MODIFIED);
        assert!(app.close());
    }

    #[test]
    fn a_close_runs_every_waiting_export_and_their_keys_first() {
        let dir = TestDir::new("export-close");
        dir.write("a.lawh", TWO_CIRCLES);
        // No changes: the window closes, after both exports.
        let mut app = opened_app_in(dir.root(), "a.lawh");
        app.feed(keys(":exp one.png<CR>:exp two.svg<CR>"));
        assert!(app.suspended.is_some());
        assert!(app.close());
        assert!(app.suspended.is_none());
        assert!(
            fs::read(dir.path("one.png"))
                .unwrap()
                .starts_with(PNG_SIGNATURE)
        );
        assert!(fs::read(dir.path("two.svg")).unwrap().starts_with(b"<?xml"));

        // Changes: both exports run, then the close warns, and the next one closes.
        let mut app = changed_app_in(dir.root(), "a.lawh");
        app.feed(keys(":exp three.pdf<CR>:exp four.pdf<CR>"));
        assert!(!app.close());
        assert!(dir.path("three.pdf").exists() && dir.path("four.pdf").exists());
        assert_eq!(app.message.as_deref(), Some(CLOSE_MODIFIED));
        assert!(app.close());

        // A `:wq` that waited behind an export closes the window once it has written the board.
        let mut app = changed_app_in(dir.root(), "a.lawh");
        app.feed(keys(":exp five.png<CR>:exp six.png<CR>:wq<CR>"));
        assert!(app.close());
        assert!(app.quitting);
        assert!(dir.path("six.png").exists());
        assert_eq!(
            fs::read_to_string(dir.path("a.lawh")).unwrap(),
            to_lawh(app.engine.board())
        );
    }

    #[test]
    fn a_failure_that_ends_the_app_runs_every_waiting_export_and_write_first() {
        let dir = TestDir::new("export-fail");
        dir.write("a.lawh", TWO_CIRCLES);
        let mut app = changed_app_in(dir.root(), "a.lawh");
        app.feed(keys(":exp a.png<CR>:w<CR>:exp b.svg<CR>"));
        assert!(app.suspended.is_some());
        app.failed(anyhow!("could not present a frame: the display is gone"));
        assert!(app.suspended.is_none());
        assert!(
            fs::read(dir.path("a.png"))
                .unwrap()
                .starts_with(PNG_SIGNATURE)
        );
        assert!(fs::read(dir.path("b.svg")).unwrap().starts_with(b"<?xml"));
        assert_eq!(
            fs::read_to_string(dir.path("a.lawh")).unwrap(),
            to_lawh(app.engine.board())
        );
        assert!(!app.engine.is_modified());
        assert_eq!(
            app.into_result().unwrap_err().to_string(),
            "could not present a frame: the display is gone"
        );
    }

    #[test]
    fn an_export_draws_the_board_as_its_whole_key_left_it() {
        // As `:w` writes it: a command after `:exp` in the same key changes what is drawn.
        let dir = TestDir::new("export-key-left");
        dir.write("a.lawh", TWO_CIRCLES);
        let mapped = |rhs: &str| {
            let mut app = opened_app_in(dir.root(), "a.lawh");
            assert_eq!(app.engine.source(&format!("nnoremap Q {rhs}\n")), []);
            app.feed(keys("Q"));
            assert_eq!(app.engine.board().len(), 1, "`x` deleted `c1`");
            app
        };

        // The selection names `c1`, which `x` deleted: nothing of it is left to draw.
        let mut app = mapped("v:exp sel.png<CR>x");
        assert_eq!(
            waiting(&app).job.selection,
            Some(vec![ObjectId::new("c1").unwrap()])
        );
        app.run_export();
        let message = app.engine.message().unwrap();
        assert!(message.error);
        assert_eq!(message.text, "cannot export \"sel.png\": nothing to export");
        assert!(!dir.path("sel.png").exists());

        // The whole board: `c2` alone is drawn.
        let mut app = mapped(":exp out.svg<CR>x");
        app.run_export();
        assert_eq!(app.engine.message().unwrap().text, "\"out.svg\" exported");
        let alone = from_lawh("lawh version=1\ncircle id=c2 x=5 y=0 r=1\n").unwrap();
        let options = ExportOptions {
            format: lawh_core::ExportFormat::Svg,
            ..ExportOptions::default()
        };
        let expected = Renderer::new_trusted()
            .export(&alone, None, &options)
            .unwrap();
        assert_eq!(fs::read(dir.path("out.svg")).unwrap(), expected.bytes);
    }

    /// The copy runs in the app crate, outside lawh-render's bench; see the dev profile in the
    /// root `Cargo.toml`. Run with `cargo test -p lawh --bin lawh -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing only; run with -- --ignored --nocapture"]
    fn copy_1080p_frame_time() {
        let rgba = vec![0x80_u8; 1920 * 1080 * 4];
        let mut out = vec![0_u32; 1920 * 1080];
        for _ in 0..3 {
            rgba_to_0rgb(&rgba, &mut out);
        }
        let mut times: Vec<_> = (0..20)
            .map(|_| {
                let t = Instant::now();
                rgba_to_0rgb(std::hint::black_box(&rgba), &mut out);
                t.elapsed()
            })
            .collect();
        times.sort();
        println!(
            "RGBA to 0RGB copy at 1920x1080: median {:.2} ms",
            ms(times[10])
        );
    }
}
