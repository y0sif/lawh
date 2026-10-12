//! The window: winit events in, frames drawn by lawh-render out through softbuffer.
//!
//! Keys go to the editing engine ([`Engine`]), which changes the board; the app does what the
//! engine asks of it ([`Effect`]): it writes the board, quits, and times out keys waiting for a
//! longer mapping. Each frame draws the board with the engine's cursor, selection, caret and
//! status line over it. The mouse pans (drag), zooms (wheel) and selects (click).

use std::collections::VecDeque;
use std::io;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow};
use lawh_core::{Cursor, Effect, Engine, Mode, Point, Rect, to_lawh};
use lawh_render::{
    Caret, Overlay, OverlayCursor, Renderer, StatusLine, Viewport, status_bar_height_px,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

use crate::{files, keys, status, view};

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
/// request closes while this still shows: a key, a click or a mapping timeout between drops
/// it, and the next close warns again.
const CLOSE_MODIFIED: &str = "unsaved changes: :w to save, :q! to discard, or close again";

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
        tracing::error!("{err:#}");
        self.error.get_or_insert(err);
        event_loop.exit();
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
        for key in translated.keys {
            let effects = self.engine.feed(key, &mut self.renderer);
            self.apply(event_loop, effects);
            if self.quitting {
                return;
            }
        }
        self.after_input();
    }

    /// The engine's timer went off: keys waiting for a longer mapping run now. As a key does, it
    /// drops the app's message, a close warning too.
    fn timeout(&mut self, event_loop: &ActiveEventLoop) {
        self.message = None;
        let effects = self.engine.timeout(&mut self.renderer);
        self.apply(event_loop, effects);
        if !self.quitting {
            self.after_input();
        }
    }

    /// Does what the engine asked for: the writes (see [`do_writes`]), then quitting or arming
    /// the timer.
    fn apply(&mut self, event_loop: &ActiveEventLoop, effects: Vec<Effect>) {
        let opened = self
            .file
            .as_ref()
            .map(|(name, real)| (name.as_str(), real.as_path()));
        let launch_dir = self.launch_dir.as_deref();
        let home = self.home.as_deref();
        let after = do_writes(
            &mut self.engine,
            effects,
            |path| files::write_target(path, opened, launch_dir, home),
            write_board,
        );
        if after.quit {
            tracing::info!("quitting");
            self.quitting = true;
            self.timer.cancel();
            event_loop.exit();
        } else if let Some(ms) = after.timer_ms {
            self.timer.arm(Instant::now(), ms);
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
        if self.close_or_warn() {
            event_loop.exit();
        } else {
            self.request_redraw();
        }
    }

    /// A close request: true when the window closes now ([`close_now`]), else the status line
    /// warns of the unwritten changes ([`CLOSE_MODIFIED`]).
    ///
    /// The close is armed only while that warning shows, so whatever drops it (a key, a click
    /// on the board, a mapping timeout) disarms it too: after anything else, a close warns
    /// first, and only two closes in a row close.
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
    fn click(&mut self, at: Point) {
        if self.quitting {
            return;
        }
        let Some((point, tolerance)) = self.viewport.and_then(|vp| click_point(&vp, at)) else {
            return;
        };
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
        self.engine.click(point, hit, &mut self.renderer);
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
    /// area.
    fn redraw(&mut self) -> anyhow::Result<()> {
        let Some(gfx) = self.gfx.as_mut() else {
            return Ok(());
        };
        let size = gfx.window.inner_size();
        let width = u16::try_from(size.width).unwrap_or(u16::MAX);
        let height = u16::try_from(size.height).unwrap_or(u16::MAX);
        let (Some(surface_w), Some(surface_h)) = (
            NonZeroU32::new(u32::from(width)),
            NonZeroU32::new(u32::from(height)),
        ) else {
            return Ok(());
        };
        if gfx.surface_size != Some((surface_w, surface_h)) {
            gfx.surface
                .resize(surface_w, surface_h)
                .map_err(|err| anyhow!("could not resize the window surface: {err}"))?;
            gfx.surface_size = Some((surface_w, surface_h));
        }

        let scale_factor = gfx.window.scale_factor();
        let vp = if let Some(vp) = self.viewport.as_mut() {
            vp.width = width;
            vp.height = height;
            vp.scale_factor = scale_factor;
            *vp
        } else {
            let bounds = self.renderer.board_bounds(self.engine.board());
            let bar = status_bar_height_px(scale_factor);
            let vp = view::opening(bounds, width, height, scale_factor, bar);
            tracing::debug!(?vp, "fitted the board to the window");
            *self.viewport.insert(vp)
        };

        let render_start = Instant::now();
        let status = status::status(&self.engine, self.message.as_deref());
        let visual = self.engine.visual_rect(&mut self.renderer);
        let text_cursor = self.engine.text_cursor();
        let overlay = Overlay {
            cursor: OverlayCursor::from(self.engine.cursor()),
            selection: self.engine.selection(),
            visual,
            caret: text_cursor.as_ref().map(Caret::from),
            status: StatusLine {
                left: &status.left,
                right: &status.right,
                error: status.error,
            },
            command_line: self.engine.command_line(),
        };
        let pixels = self
            .renderer
            .render_with(self.engine.board(), &vp, &overlay);
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

        self.frames += 1;
        // Objects over the renderer's work budget are drawn as outlines; it logs how many.
        let drawn = self.renderer.last_frame();
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
        // Text the frame couldn't afford to lay out is drawn by the next one. Math still
        // compiling asks for a frame itself, through the engine's waker.
        if drawn.incomplete() {
            gfx.window.request_redraw();
        }
        Ok(())
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
        if self.timer.fire(Instant::now()) {
            self.timeout(event_loop);
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
            WindowEvent::MouseInput { state, button, .. } => self.mouse_input(state, button),
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
        event_loop.set_control_flow(self.timer.control_flow());
    }
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

/// What is left of a batch of effects once its writes are done.
#[derive(Debug, Default, PartialEq, Eq)]
struct AfterWrites {
    /// The engine allows quitting ([`Effect::Quit`]).
    quit: bool,
    /// Arm the engine's timer for this many milliseconds ([`Effect::ArmTimer`]).
    timer_ms: Option<u64>,
}

/// Does the writes `effects` ask for, those of one key or timeout, reporting each to `engine`
/// in order with [`Engine::write_done`] (which may add [`Effect::Quit`] after them), and returns
/// what else the effects ask for. `target` gives the file a write's path names; `write` writes
/// it. The writes go through one [`files::WriteBatch`]: the board is turned into text once and
/// each file is written once, but every write is reported, so the engine's writes waiting for
/// their reports all get one. Stops at [`Effect::Quit`].
fn do_writes(
    engine: &mut Engine,
    effects: Vec<Effect>,
    target: impl Fn(&str) -> PathBuf,
    mut write: impl FnMut(&Path, &[u8]) -> io::Result<()>,
) -> AfterWrites {
    let mut batch = files::WriteBatch::default();
    let mut after = AfterWrites::default();
    let mut queue = VecDeque::from(effects);
    while let Some(effect) = queue.pop_front() {
        match effect {
            Effect::Write { path, .. } => {
                let board = engine.board();
                let result = batch.write(target(&path), || to_lawh(board), &mut write);
                queue.extend(engine.write_done(&path, result));
            }
            Effect::Quit => {
                after.quit = true;
                break;
            }
            Effect::ArmTimer { ms } => after.timer_ms = Some(ms),
        }
    }
    after
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
mod tests {
    use lawh_core::engine::MAX_WRITES_PER_KEY;
    use lawh_core::{FixedMeasure, from_lawh, parse_keys};

    use super::*;

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
        let after = do_writes(&mut engine, effects, as_given, |target, bytes| {
            written.push((target.to_owned(), bytes.to_vec()));
            Ok(())
        });
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
        let after = do_writes(&mut engine, effects, as_given, |_, _| {
            tries += 1;
            Err(io::Error::other("disk full"))
        });
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
        let after = do_writes(&mut engine, effects, as_given, |target, _| {
            if target == Path::new(copy) {
                Err(io::Error::other("no such directory"))
            } else {
                Ok(())
            }
        });
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
        let after = do_writes(&mut engine, effects, as_given, |_, _| Ok(()));
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
            &Cursor::Object(lawh_core::ObjectId::new("l1").unwrap())
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
            &Cursor::Object(lawh_core::ObjectId::new("t1").unwrap())
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
