//! The window: winit events in, frames drawn by lawh-render out through softbuffer.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow};
use lawh_core::{Board, Point};
use lawh_render::{Renderer, Viewport};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::view;

/// Window size on open, in logical pixels.
const INITIAL_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 800.0);

/// Sent to the event loop by the math engine's waker when a formula finishes compiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MathReady;

/// The open board and its window.
pub struct App {
    board: Board,
    title: String,
    /// Draws the board; its math engine never blocks a frame, and its work budget keeps frames
    /// short whatever the board holds.
    renderer: Renderer,
    /// Created on the first `resumed`.
    gfx: Option<Gfx>,
    /// `None` until the next frame fits the board in the window; after that it holds the user's
    /// pan and zoom.
    viewport: Option<Viewport>,
    /// The user has panned or zoomed. Until then the view is fitted again whenever a formula
    /// finishes, since its real size replaces its placeholder's.
    view_moved: bool,
    /// Last cursor position in physical pixels, once the cursor has been over the window.
    cursor: Option<Point>,
    left_down: bool,
    middle_down: bool,
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

impl App {
    pub fn new(board: Board, renderer: Renderer, title: String, started: Instant) -> Self {
        Self {
            board,
            title,
            renderer,
            gfx: None,
            viewport: None,
            view_moved: false,
            cursor: None,
            left_down: false,
            middle_down: false,
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

    fn dragging(&self) -> bool {
        self.left_down || self.middle_down
    }

    fn cursor_moved(&mut self, to: Point) {
        let from = self.cursor.replace(to);
        if !self.dragging() {
            return;
        }
        if let (Some(from), Some(vp)) = (from, self.viewport.as_mut()) {
            view::pan(vp, from, to);
            self.view_moved = true;
            self.request_redraw();
        }
    }

    /// A formula finished compiling: draw it, fitting the view again if the user hasn't moved it.
    fn math_ready(&mut self) {
        if !self.view_moved && self.viewport.take().is_some() {
            tracing::debug!("math finished; fitting the board to the window again");
        }
        self.request_redraw();
    }

    fn mouse_input(&mut self, state: ElementState, button: MouseButton) {
        let down = state.is_pressed();
        match button {
            MouseButton::Left => self.left_down = down,
            MouseButton::Middle => self.middle_down = down,
            _ => {}
        }
    }

    fn zoom(&mut self, factor: f64) {
        let Some(vp) = self.viewport.as_mut() else {
            return;
        };
        let at = self.cursor.unwrap_or(Point::new(
            f64::from(vp.width) / 2.0,
            f64::from(vp.height) / 2.0,
        ));
        view::zoom_at(vp, at, factor);
        self.view_moved = true;
        self.request_redraw();
    }

    /// Draws the board and presents it. Does nothing while the window has no area.
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
            let vp = match self.renderer.board_bounds(&self.board) {
                Some(bounds) => Viewport::fit(bounds, width, height, scale_factor),
                None => Viewport::new(Point::ZERO, 1.0, width, height, scale_factor),
            };
            tracing::debug!(?vp, "fitted the board to the window");
            *self.viewport.insert(vp)
        };

        let render_start = Instant::now();
        let pixels = self.renderer.render(&self.board, &vp);
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
            WindowEvent::CloseRequested => event_loop.exit(),
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
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_moved(Point::new(position.x, position.y));
            }
            WindowEvent::MouseInput { state, button, .. } => self.mouse_input(state, button),
            WindowEvent::MouseWheel { delta, .. } => {
                let scale_factor = self.viewport.map_or(1.0, |vp| vp.scale_factor);
                self.zoom(view::wheel_zoom_factor(delta, scale_factor));
            }
            // A release that happens while another window has focus never arrives here.
            WindowEvent::Focused(false) => {
                self.left_down = false;
                self.middle_down = false;
            }
            _ => {}
        }
    }
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

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_become_0rgb() {
        let rgba = [0x11, 0x22, 0x33, 0xFF, 0xFA, 0xFA, 0xF7, 0xFF];
        let mut out = [0xDEAD_BEEF; 2];
        rgba_to_0rgb(&rgba, &mut out);
        assert_eq!(out, [0x0011_2233, 0x00FA_FAF7]);
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
