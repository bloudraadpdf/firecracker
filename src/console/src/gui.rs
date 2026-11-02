// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::io;
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::auth::Password;
use crate::graphics::Graphics;
use crate::input::{self, Held, Motion, Viewport};
use crate::{Frame, client, invalid};

const TITLE: &str = "Firecracker | Ctrl+Alt: Home Windows | End Ctrl+Alt+Del | Enter fullscreen";

/// Connection options for a native console viewer. Use SSH forwarding for remote hosts.
#[derive(Debug)]
pub struct ViewerOptions {
    /// A loopback RFB bridge address.
    pub address: SocketAddr,
    /// A file containing the bridge's VNC password, when authentication is enabled.
    pub password_file: Option<PathBuf>,
}

enum Event {
    Frame,
    Failed(io::Error),
}
enum Command {
    Key(u32, bool),
    Pointer(u8, (u16, u16)),
    Release,
    Shortcut(&'static [u32]),
}

struct Connection {
    stream: TcpStream,
    stop: Arc<AtomicBool>,
    commands: mpsc::SyncSender<Command>,
    latest: Arc<Mutex<Option<Frame>>>,
    threads: Vec<JoinHandle<()>>,
}

impl Connection {
    fn open(
        options: ViewerOptions,
        notify: impl Fn(Event) + Send + Sync + 'static,
    ) -> io::Result<(Self, Frame)> {
        if !options.address.ip().is_loopback() {
            return Err(invalid(
                "viewer requires a loopback address; use SSH forwarding",
            ));
        }
        let password = options
            .password_file
            .map(|path| Password::from_file(&path))
            .transpose()?;
        let mut stream = TcpStream::connect_timeout(&options.address, Duration::from_secs(2))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let initial = client::handshake(&mut stream, password.as_ref())?;
        let (commands, receiver) = mpsc::sync_channel(128);
        let mut connection = Self {
            stream: stream.try_clone()?,
            stop: Arc::new(AtomicBool::new(false)),
            commands,
            latest: Arc::new(Mutex::new(None)),
            threads: Vec::new(),
        };
        let writer = Arc::new(Mutex::new(stream.try_clone()?));
        let input = writer.clone();
        let notify = Arc::new(notify);
        let stop = connection.stop.clone();
        let errors = notify.clone();
        connection.threads.push(
            std::thread::Builder::new()
                .name("console-input".into())
                .spawn(move || {
                    let mut held = Held::default();
                    let result = (|| {
                        while !stop.load(Ordering::Acquire) {
                            let command = match receiver.recv_timeout(Duration::from_millis(50)) {
                                Ok(command) => command,
                                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            };
                            let mut writer = input.lock().unwrap();
                            match command {
                                Command::Key(key, down) => held.key(&mut *writer, key, down)?,
                                Command::Pointer(buttons, point) => {
                                    held.pointer(&mut *writer, buttons, point)?
                                }
                                Command::Release => held.release(&mut *writer)?,
                                Command::Shortcut(keys) => held.shortcut(&mut *writer, keys)?,
                            }
                        }
                        held.release(&mut *input.lock().unwrap())
                    })();
                    if let Err(error) = result
                        && !stop.load(Ordering::Acquire)
                    {
                        errors(Event::Failed(error));
                    }
                    let _ = input.lock().unwrap().shutdown(Shutdown::Both);
                })?,
        );
        let stop = connection.stop.clone();
        let latest = connection.latest.clone();
        let mut previous = initial.clone();
        connection.threads.push(
            std::thread::Builder::new()
                .name("console-display".into())
                .spawn(move || {
                    let result = (|| {
                        while !stop.load(Ordering::Acquire) {
                            let started = std::time::Instant::now();
                            client::request(
                                &mut *writer.lock().unwrap(),
                                (previous.width, previous.height),
                            )?;
                            let next = client::frame(&mut stream, &mut previous)?;
                            let mut slot = latest.lock().unwrap();
                            let changed = slot.is_none();
                            *slot = Some(next);
                            drop(slot);
                            if changed {
                                notify(Event::Frame);
                            }
                            std::thread::sleep(
                                Duration::from_millis(33).saturating_sub(started.elapsed()),
                            );
                        }
                        Ok::<_, io::Error>(())
                    })();
                    if let Err(error) = result
                        && !stop.load(Ordering::Acquire)
                    {
                        notify(Event::Failed(error));
                    }
                })?,
        );
        Ok((connection, initial))
    }

    fn disconnect(&self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.disconnect();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

struct Viewer<'a> {
    connection: Connection,
    frame: Frame,
    window: Option<Arc<Window>>,
    graphics: Option<Graphics>,
    keys: HashMap<PhysicalKey, u32>,
    modifiers: ModifiersState,
    buttons: u8,
    point: Option<(u16, u16)>,
    cursor: Option<(i32, i32)>,
    motion: Motion,
    error: Option<io::Error>,
    disconnected: Option<io::Error>,
    child: Option<&'a mut std::process::Child>,
}

impl Viewer<'_> {
    fn title(&self) -> String {
        if let Some(error) = &self.disconnected {
            format!(
                "Firecracker | Console disconnected: {error} | Waiting for VMM; close powers off \
                 VM"
            )
        } else if self.child.is_some() {
            format!("{TITLE} | Close powers off VM")
        } else {
            TITLE.into()
        }
    }
    fn update_pointer(&mut self) {
        let window = self.window.as_ref().unwrap();
        let size = window.inner_size();
        self.point = self
            .cursor
            .filter(|_| self.disconnected.is_none())
            .and_then(|(x, y)| {
                Viewport::fit(
                    (size.width, size.height),
                    (self.frame.width, self.frame.height),
                )
                .and_then(|v| v.point(i64::from(x), i64::from(y), self.buttons != 0))
            });
        window.set_cursor_visible(self.point.is_none());
    }
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: io::Error) {
        self.error = Some(error);
        event_loop.exit();
    }
    fn connection_failed(&mut self, event_loop: &ActiveEventLoop, error: io::Error) {
        if self.child.is_none() {
            self.fail(event_loop, error);
        } else if self.disconnected.is_none() {
            self.connection.disconnect();
            self.motion.cancel();
            self.keys.clear();
            self.buttons = 0;
            self.disconnected = Some(error);
            if let Some(window) = &self.window {
                window.set_cursor_visible(true);
                window.set_title(&self.title());
            }
        }
    }
    fn send(&mut self, event_loop: &ActiveEventLoop, command: Command) {
        if self.disconnected.is_some() {
            return;
        }
        if self.connection.commands.try_send(command).is_err() {
            self.connection_failed(
                event_loop,
                invalid("console input queue stopped or full; disconnected to release input"),
            );
        }
    }
}

impl ApplicationHandler<Event> for Viewer<'_> {
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.motion.take(std::time::Instant::now()) {
            self.update_pointer();
            if let Some(point) = self.point {
                self.send(event_loop, Command::Pointer(self.buttons, point));
            }
        }
        let mut wake = self.motion.deadline();
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => event_loop.exit(),
                Ok(Some(status)) => self.fail(
                    event_loop,
                    io::Error::other(format!("VMM exited: {status}")),
                ),
                Err(error) => self.fail(event_loop, error),
                Ok(None) => {
                    let child_poll = std::time::Instant::now() + Duration::from_millis(100);
                    wake = Some(wake.map_or(child_poll, |motion| motion.min(child_poll)));
                }
            }
        }
        event_loop.set_control_flow(wake.map_or(
            winit::event_loop::ControlFlow::Wait,
            winit::event_loop::ControlFlow::WaitUntil,
        ));
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.title())
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.frame.width,
                self.frame.height,
            ));
        #[cfg(target_os = "macos")]
        let attributes = {
            use winit::platform::macos::{OptionAsAlt, WindowAttributesExtMacOS};
            attributes.with_option_as_alt(OptionAsAlt::Both)
        };
        let result = (|| {
            let window = Arc::new(
                event_loop
                    .create_window(attributes)
                    .map_err(io::Error::other)?,
            );
            let mut graphics = Graphics::new(window.clone())?;
            graphics.upload(&self.frame)?;
            window.request_redraw();
            self.window = Some(window);
            self.graphics = Some(graphics);
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(event_loop, error);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        match event {
            Event::Failed(error) => self.connection_failed(event_loop, error),
            Event::Frame => {
                let frame = self.connection.latest.lock().unwrap().take();
                if let Some(frame) = frame {
                    self.frame = frame;
                    if self.window.is_some() {
                        self.update_pointer();
                    }
                    if let Some(graphics) = &mut self.graphics
                        && let Err(error) = graphics.upload(&self.frame)
                    {
                        self.fail(event_loop, error);
                        return;
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_none_or(|window| window.id() != id) {
            return;
        }
        if self.disconnected.is_some()
            && !matches!(
                &event,
                WindowEvent::CloseRequested
                    | WindowEvent::RedrawRequested
                    | WindowEvent::Resized(_)
            )
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.buttons = 0;
                self.cursor = None;
                self.motion.cancel();
                self.update_pointer();
                self.modifiers = ModifiersState::empty();
                self.send(event_loop, Command::Release);
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                for (key, down) in input::modifiers(self.modifiers, &mut self.keys, false) {
                    self.send(event_loop, Command::Key(key, down));
                }
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                let down = event.state == ElementState::Pressed;
                if down && self.modifiers.control_key() && self.modifiers.alt_key() {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Home) => {
                            if !event.repeat {
                                self.send(event_loop, Command::Shortcut(&[0xffeb]));
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::End) => {
                            if !event.repeat {
                                self.send(event_loop, Command::Shortcut(&[0xffe3, 0xffe9, 0xffff]));
                            }
                            return;
                        }
                        PhysicalKey::Code(KeyCode::Enter) => {
                            if !event.repeat
                                && let Some(window) = &self.window
                            {
                                window.set_fullscreen(if window.fullscreen().is_some() {
                                    None
                                } else {
                                    Some(Fullscreen::Borderless(None))
                                });
                            }
                            return;
                        }
                        _ => (),
                    }
                }
                let key = if down {
                    use winit::keyboard::{Key, NamedKey};
                    if !matches!(
                        event.logical_key,
                        Key::Named(
                            NamedKey::Shift
                                | NamedKey::Control
                                | NamedKey::Alt
                                | NamedKey::Super
                                | NamedKey::Meta
                        )
                    ) {
                        for (key, down) in input::modifiers(self.modifiers, &mut self.keys, true) {
                            self.send(event_loop, Command::Key(key, down));
                        }
                    }
                    input::keysym(&event.logical_key, event.physical_key)
                        .map(|sym| *self.keys.entry(event.physical_key).or_insert(sym))
                } else {
                    self.keys.remove(&event.physical_key)
                };
                if let Some(key) = key {
                    self.send(event_loop, Command::Key(key, down));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let position = position.cast::<i32>();
                self.cursor = Some((position.x, position.y));
                self.update_pointer();
                if self.point.is_some() {
                    self.motion.schedule();
                } else {
                    self.motion.cancel();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.update_pointer();
                self.motion.cancel();
                let mask = match button {
                    MouseButton::Left => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Right => 4,
                    _ => return,
                };
                if let Some(point) = self.point {
                    if state == ElementState::Pressed {
                        self.buttons |= mask;
                    } else {
                        self.buttons &= !mask;
                    }
                    self.send(event_loop, Command::Pointer(self.buttons, point));
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.update_pointer();
                self.motion.cancel();
                let y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                    MouseScrollDelta::PixelDelta(point) => point.y,
                };
                if y != 0.0
                    && let Some(point) = self.point
                {
                    let wheel = if y > 0.0 { 8 } else { 16 };
                    self.send(event_loop, Command::Pointer(self.buttons | wheel, point));
                    self.send(event_loop, Command::Pointer(self.buttons, point));
                }
            }
            WindowEvent::Resized(_) => {
                self.update_pointer();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(graphics) = &mut self.graphics
                    && let Err(error) = graphics.draw()
                {
                    self.fail(event_loop, error);
                }
            }
            WindowEvent::CursorLeft { .. } if self.buttons == 0 => {
                self.cursor = None;
                self.motion.cancel();
                self.update_pointer();
            }
            _ => (),
        }
    }
}

/// Display an existing RFB console. Closing this viewer leaves the VM running.
pub fn view(options: ViewerOptions) -> io::Result<()> {
    view_inner(options, None)
}

/// Display a console and close the window when its VMM exits. The caller owns child cleanup.
pub fn view_with_child(options: ViewerOptions, child: &mut std::process::Child) -> io::Result<()> {
    view_inner(options, Some(child))
}

fn view_inner(options: ViewerOptions, child: Option<&mut std::process::Child>) -> io::Result<()> {
    let mut builder = EventLoop::<Event>::with_user_event();
    #[cfg(all(test, target_os = "linux"))]
    {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_x11().with_any_thread(true);
    }
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        builder.with_default_menu(false);
    }
    let event_loop = builder.build().map_err(io::Error::other)?;
    let proxy = event_loop.create_proxy();
    let (connection, frame) = Connection::open(options, move |event| {
        let _ = proxy.send_event(event);
    })?;
    let mut viewer = Viewer {
        connection,
        frame,
        window: None,
        graphics: None,
        keys: HashMap::new(),
        modifiers: ModifiersState::empty(),
        buttons: 0,
        point: None,
        cursor: None,
        motion: Motion::default(),
        error: None,
        disconnected: None,
        child,
    };
    event_loop.run_app(&mut viewer).map_err(io::Error::other)?;
    viewer.error.map_or(Ok(()), Err)
}

#[cfg(test)]
#[path = "gui_tests.rs"]
mod tests;
