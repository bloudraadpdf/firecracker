// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::ffi::CStr;
use std::process::{Command as Process, Stdio};
use std::ptr;

use x11_dl::{xlib, xtest};

use super::*;

struct Display {
    api: xlib::Xlib,
    handle: *mut xlib::Display,
    root: xlib::Window,
}

impl Display {
    fn open() -> Self {
        let api = xlib::Xlib::open().unwrap();
        // SAFETY: XOpenDisplay owns the connection until this object's Drop.
        let handle = unsafe { (api.XOpenDisplay)(ptr::null()) };
        assert!(!handle.is_null(), "requires an X11 display");
        // SAFETY: handle is a live display connection.
        let root = unsafe { (api.XDefaultRootWindow)(handle) };
        Self { api, handle, root }
    }

    fn title(&self, window: xlib::Window) -> String {
        let mut name = ptr::null_mut();
        // SAFETY: XFetchName returns an Xlib allocation, freed after copying.
        unsafe {
            if (self.api.XFetchName)(self.handle, window, &mut name) == 0 || name.is_null() {
                return String::new();
            }
            let title = CStr::from_ptr(name).to_string_lossy().into_owned();
            (self.api.XFree)(name.cast());
            title
        }
    }

    fn window(&self) -> Option<xlib::Window> {
        let mut root = 0;
        let mut parent = 0;
        let mut children = ptr::null_mut();
        let mut count = 0;
        // SAFETY: XQueryTree initialises children/count; copy before freeing its allocation.
        let windows = unsafe {
            assert_ne!(
                (self.api.XQueryTree)(
                    self.handle,
                    self.root,
                    &mut root,
                    &mut parent,
                    &mut children,
                    &mut count
                ),
                0
            );
            let windows = if count == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(children, count as usize).to_vec()
            };
            if !children.is_null() {
                (self.api.XFree)(children.cast());
            }
            windows
        };
        windows
            .into_iter()
            .find(|&window| self.title(window).starts_with("Firecracker |"))
    }

    fn close(&self, window: xlib::Window) {
        // SAFETY: the event contains the WM_DELETE_WINDOW atom for this live window/display.
        unsafe {
            let mut data = xlib::ClientMessageData::new();
            data.set_long(
                0,
                (self.api.XInternAtom)(self.handle, c"WM_DELETE_WINDOW".as_ptr(), 0).cast_signed(),
            );
            let mut event = xlib::XEvent {
                client_message: xlib::XClientMessageEvent {
                    type_: xlib::ClientMessage,
                    serial: 0,
                    send_event: 1,
                    display: self.handle,
                    window,
                    message_type: (self.api.XInternAtom)(self.handle, c"WM_PROTOCOLS".as_ptr(), 0),
                    format: 32,
                    data,
                },
            };
            assert_ne!(
                (self.api.XSendEvent)(self.handle, window, 0, 0, &mut event),
                0
            );
            (self.api.XFlush)(self.handle);
        }
    }

    fn press_and_drag(&self, window: xlib::Window, messages: &Mutex<Vec<Message>>) {
        let input = xtest::Xf86vmode::open().unwrap();
        // SAFETY: all Xlib/XTest calls use this live display and the viewer's live window.
        unsafe {
            (self.api.XMoveResizeWindow)(self.handle, window, 0, 0, 800, 600);
            (self.api.XSetInputFocus)(self.handle, window, xlib::RevertToParent, xlib::CurrentTime);
            (input.XTestFakeMotionEvent)(self.handle, -1, 20, 20, 0);
            (self.api.XSync)(self.handle, 0);
        }
        wait_until(|| {
            messages
                .lock()
                .unwrap()
                .contains(&Message::Pointer(0, 0, 0))
        });
        // SAFETY: the display remains live; the server resolves the keycode for ASCII a.
        unsafe {
            let key = (self.api.XKeysymToKeycode)(self.handle, 0x61);
            (input.XTestFakeKeyEvent)(self.handle, key.into(), 1, 0);
            (input.XTestFakeButtonEvent)(self.handle, 1, 1, 0);
            (input.XTestFakeMotionEvent)(self.handle, -1, 1000, 700, 0);
            (self.api.XSync)(self.handle, 0);
        }
        wait_until(|| {
            messages
                .lock()
                .unwrap()
                .contains(&Message::Pointer(1, 3, 2))
        });
        // SAFETY: move focus to the live root window to exercise native FocusOut.
        unsafe {
            (self.api.XSetInputFocus)(
                self.handle,
                self.root,
                xlib::RevertToParent,
                xlib::CurrentTime,
            );
            (self.api.XSync)(self.handle, 0);
        }
        wait_until(|| {
            let messages = messages.lock().unwrap();
            messages.contains(&Message::Key(0x61, false))
                && messages.contains(&Message::Pointer(0, 3, 2))
        });
        // SAFETY: release the X server's physical input after testing focus-loss release.
        unsafe {
            let key = (self.api.XKeysymToKeycode)(self.handle, 0x61);
            (input.XTestFakeKeyEvent)(self.handle, key.into(), 0, 0);
            (input.XTestFakeButtonEvent)(self.handle, 1, 0, 0);
            (self.api.XSync)(self.handle, 0);
        }
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: this is the connection returned by XOpenDisplay and it is closed once.
        unsafe {
            (self.api.XCloseDisplay)(self.handle);
        }
    }
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "native GUI event did not complete"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires X11, XTest and a Vulkan driver; run under Xvfb/lavapipe"]
fn native_window_lifecycle_and_input() {
    const SCENARIO: &str = "FIRECRACKER_GUI_TEST_SCENARIO";
    let Ok(scenario) = std::env::var(SCENARIO) else {
        for scenario in [
            "close-input",
            "owned-success",
            "owned-failure",
            "attached-disconnect",
        ] {
            let mut child = Process::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "gui::tests::x11::native_window_lifecycle_and_input",
                    "--ignored",
                    "--nocapture",
                ])
                .env(SCENARIO, scenario)
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(20);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break Some(status);
                }
                if Instant::now() >= deadline {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            let _ = child.kill();
            child.wait().unwrap();
            assert!(
                status.expect("native GUI subprocess timed out").success(),
                "{scenario}"
            );
        }
        return;
    };
    let server = Server::new();
    let owned = scenario.starts_with("owned-");
    let mut child = owned.then(|| {
        Process::new("sh")
            .args([
                "-c",
                if scenario == "owned-success" {
                    "read line; exit 0"
                } else {
                    "read line; exit 7"
                },
            ])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap()
    });
    let mut control = child.as_mut().and_then(|child| child.stdin.take());
    let peer = server.peer.clone();
    let messages = server.messages.clone();
    let case = scenario.clone();
    let controller = std::thread::spawn(move || {
        let display = Display::open();
        let mut window = None;
        wait_until(|| {
            window = display.window();
            window.is_some()
        });
        let window = window.unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wait_until(|| messages.lock().unwrap().contains(&Message::Refresh));
            if case == "close-input" {
                display.press_and_drag(window, &messages);
            } else {
                peer.lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .shutdown(Shutdown::Both)
                    .unwrap();
                if owned {
                    wait_until(|| display.title(window).contains("Waiting for VMM"));
                    control.as_mut().unwrap().write_all(b"exit\n").unwrap();
                }
            }
        }));
        if (case == "close-input" || result.is_err()) && display.window() == Some(window) {
            display.close(window);
        }
        result.unwrap();
    });
    let result = view_inner(server.options(), child.as_mut());
    if let Some(child) = &mut child {
        let _ = child.kill();
        child.wait().unwrap();
    }
    controller.join().unwrap();
    match scenario.as_str() {
        "close-input" | "owned-success" => result.unwrap(),
        "owned-failure" => assert!(result.unwrap_err().to_string().contains("VMM exited")),
        "attached-disconnect" => assert!(result.is_err()),
        _ => unreachable!(),
    }
}
