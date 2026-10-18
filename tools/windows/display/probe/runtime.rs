// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use core::cell::RefCell;
use core::fmt::{self, Write};
use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicU32, Ordering};

use crate::abi::*;
use crate::model::{self, Action, CursorBalance, Engine, Error, Mode, Phase, Point};
#[path = "../../egress/win32.rs"]
mod win32;

struct Data {
    device: [u16; 32],
    original: DevMode,
    modes: [DevMode; model::CASES],
    cursor: CursorBalance,
    timer: usize,
    mode_changed: bool,
}
struct Session {
    owner: AtomicU32,
    engine: Engine,
    data: RefCell<Data>,
    log_bytes: AtomicU32,
}
// SAFETY: Private non-atomic fields are accessed only after the entry or callback owner check.
// Foreign callbacks are rejected, and all state borrows end before FFI re-entry.
unsafe impl Sync for Session {}
static SESSION: Session = Session {
    owner: AtomicU32::new(0),
    engine: Engine::new(),
    data: RefCell::new(Data {
        device: [0; 32],
        original: DevMode::empty(),
        modes: [DevMode::empty(); model::CASES],
        cursor: CursorBalance::new(),
        timer: 0,
        mode_changed: false,
    }),
    log_bytes: AtomicU32::new(0),
};
static mut PIXELS: [u32; model::MAX_PIXELS] = [0; model::MAX_PIXELS];
fn owner() {
    // SAFETY: The API takes no pointers; its thread ID is read before any private GUI state access.
    if unsafe { GetCurrentThreadId() } != SESSION.owner.load(Ordering::Relaxed) {
        // SAFETY: The DWORD exit code is valid; process termination is intentional and is not a restore claim.
        unsafe { win32::ExitProcess(251) };
    }
}
const MAX_LOG_BYTES: usize = 1024 * 1024;
struct Output;
impl Write for Output {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        owner();
        let length = u32::try_from(value.len()).map_err(|_| fmt::Error)?;
        let total = SESSION.log_bytes.fetch_add(length, Ordering::Relaxed);
        if total as usize > MAX_LOG_BYTES || value.len() > MAX_LOG_BYTES - total as usize {
            return Err(fmt::Error);
        }
        let mut written = 0;
        // SAFETY: The input bytes and writable count live through this synchronous call; no OVERLAPPED or state borrow is passed.
        let result = unsafe {
            win32::WriteFile(
                win32::GetStdHandle(0xffff_fff5),
                value.as_ptr(),
                length,
                &mut written,
                null_mut(),
            )
        };
        if result == 0 || written as usize != value.len() {
            Err(fmt::Error)
        } else {
            Ok(())
        }
    }
}
fn log(value: fmt::Arguments<'_>) {
    if Output.write_fmt(value).is_err() {
        SESSION.engine.abort(Error::Log);
    }
}
fn last_error(api: &str, success: bool) -> bool {
    if !success {
        // SAFETY: This pointer-free API reads only the current thread error code.
        let error = unsafe { win32::GetLastError() };
        log(format_args!("API {} error={}\r\n", api, error));
    }
    success
}
fn enum_mode(device: &[u16; 32], index: u32) -> Option<DevMode> {
    let mut mode = DevMode::empty();
    let discovery = SESSION.engine.view().phase == Phase::Boot;
    // SAFETY: The device name is NUL-terminated; the writable 220-byte public mode has zero private capacity and outlives the call.
    let result = unsafe { EnumDisplaySettingsW(device.as_ptr(), index, &mut mode) };
    if discovery {
        // SAFETY: This pointer-free read precedes logging; the value is informational, not a documented EnumDisplaySettingsW error contract.
        let error = unsafe { win32::GetLastError() };
        log(format_args!(
            "DISCOVERY_MODE index={} result={} last_error_informational={} value={:?}\r\n",
            index, result, error, mode
        ));
    }
    if result == 0 {
        return None;
    }
    Some(mode)
}
fn discover() -> bool {
    let mut primary = None;
    for index in 0..32 {
        let mut device = DisplayDevice::empty();
        // SAFETY: The 840-byte writable device record has cb set; NULL requests adapter enumeration with no device string.
        let result = unsafe { EnumDisplayDevicesW(null(), index, &mut device, 0) };
        // SAFETY: This pointer-free read immediately follows enumeration, before any logging; it is informational and may be stale.
        let error = unsafe { win32::GetLastError() };
        log(format_args!(
            "DISCOVERY_ADAPTER index={} result={} last_error_informational={} size={} flags={:x}\r\n",
            index, result, error, device.size, device.flags
        ));
        if result == 0 {
            break;
        }
        log(format_args!(
            "ADAPTER index={} flags={:x} name={:?} description={:?}\r\n",
            index, device.flags, device.name, device.description
        ));
        if device.flags & 5 == 5 && primary.replace(device.name).is_some() {
            log(format_args!(
                "DISCOVERY_REJECT reason=duplicate-primary index={}\r\n",
                index
            ));
            return false;
        }
        if index == 31 {
            log(format_args!("DISCOVERY_REJECT reason=adapter-limit\r\n"));
            return false;
        }
    }
    let Some(device) = primary else {
        log(format_args!("DISCOVERY_REJECT reason=no-primary\r\n"));
        return false;
    };
    let Some(original) = enum_mode(&device, u32::MAX) else {
        log(format_args!("DISCOVERY_REJECT reason=current-query\r\n"));
        return false;
    };
    let Some(registry) = enum_mode(&device, u32::MAX - 1) else {
        log(format_args!("DISCOVERY_REJECT reason=registry-query\r\n"));
        return false;
    };
    if !original.valid()
        || !registry.valid()
        || original.position != Point::default()
        || original.orientation != 0
    {
        log(format_args!(
            "DISCOVERY_REJECT reason=mode-guard current_valid={} registry_valid={} position_zero={} orientation_zero={}\r\n",
            original.valid(),
            registry.valid(),
            original.position == Point::default(),
            original.orientation == 0
        ));
        return false;
    }
    log(format_args!(
        "ORIGINAL {} {} {} {} registry={} {} {} {}\r\n",
        original.width,
        original.height,
        original.bits,
        original.frequency,
        registry.width,
        registry.height,
        registry.bits,
        registry.frequency
    ));
    SESSION.data.borrow_mut().device = device;
    SESSION.data.borrow_mut().original = original;
    let mut found = [false; model::CASES];
    for index in 0..256 {
        let Some(mode) = enum_mode(&device, index) else {
            let complete = found.iter().all(|value| *value);
            log(format_args!(
                "DISCOVERY_ENUM_END index={} complete={} found={:?}\r\n",
                index, complete, found
            ));
            if !complete {
                log(format_args!("DISCOVERY_REJECT reason=missing-modes\r\n"));
            }
            return complete;
        };
        if !mode.valid() {
            log(format_args!(
                "DISCOVERY_REJECT reason=enumerated-mode-invalid index={}\r\n",
                index
            ));
            return false;
        }
        log(format_args!(
            "ENUM index={} size={} private={} fields={:x} geometry={}x{} bits={} frequency={} flags={:x}\r\n",
            index,
            mode.size,
            mode.driver_extra,
            mode.fields,
            mode.width,
            mode.height,
            mode.bits,
            mode.frequency,
            mode.flags
        ));
        for (case, present) in found.iter_mut().enumerate() {
            if Mode::at(case).is_some_and(|expected| mode.matches(expected)) && !*present {
                SESSION.data.borrow_mut().modes[case] = mode;
                *present = true;
            }
        }
    }
    log(format_args!("DISCOVERY_REJECT reason=mode-limit\r\n"));
    false
}
fn cursor_info() -> Option<CursorInfo> {
    let mut info = CursorInfo::empty();
    // SAFETY: The local writable cursor record has its target-sized cb field set and stays alive for the call.
    last_error("GetCursorInfo", unsafe { GetCursorInfo(&mut info) } != 0).then_some(info)
}
fn hide_cursor() -> bool {
    if cursor_info().is_none_or(|info| info.flags != 1) {
        return false;
    }
    if SESSION.data.borrow().cursor.owned != 0 {
        return false;
    }
    SESSION.data.borrow_mut().cursor = CursorBalance::default();
    loop {
        // SAFETY: Only this owner thread changes its cursor count; the return is recorded for each bounded owned decrement.
        let value = unsafe { ShowCursor(0) };
        let hidden = SESSION.data.borrow_mut().cursor.hide(value);
        let owned = SESSION.data.borrow().cursor.owned;
        log(format_args!(
            "CURSOR hide returned={} owned={}\r\n",
            value, owned
        ));
        if hidden {
            return cursor_info().is_some_and(|info| info.flags == 0);
        }
        if !SESSION.data.borrow().cursor.can_hide_more() {
            return false;
        }
    }
}
fn show_cursor() -> bool {
    let mut valid = true;
    loop {
        if SESSION.data.borrow().cursor.owned == 0 {
            break;
        }
        // SAFETY: Only this owner thread changes its cursor count; the return is recorded for each bounded owned increment.
        let value = unsafe { ShowCursor(1) };
        valid &= SESSION.data.borrow_mut().cursor.restored(value);
        let owned = SESSION.data.borrow().cursor.owned;
        log(format_args!(
            "CURSOR restore returned={} owned={}\r\n",
            value, owned
        ));
    }
    let balance = SESSION.data.borrow().cursor;
    valid && balance.balanced() && cursor_info().is_some_and(|info| info.flags == 1)
}
fn point() -> Option<Point> {
    let mut point = Point::default();
    // SAFETY: The local repr(C) POINT is writable, correctly sized and not borrowed from session state.
    last_error("GetCursorPos", unsafe { GetCursorPos(&mut point) } != 0).then_some(point)
}
fn apply(window: usize, case: u8) -> bool {
    let Some(expected) = Mode::at(case.into()) else {
        return false;
    };
    let (Ok(width), Ok(height)) = (
        i32::try_from(expected.width),
        i32::try_from(expected.height),
    ) else {
        return false;
    };
    let (device, mut mode) = {
        let d = SESSION.data.borrow();
        let Some(mode) = d.modes.get(case as usize).copied() else {
            return false;
        };
        (d.device, mode)
    };
    // SAFETY: The NUL-terminated device and copied public DEVMODE stay live; private capacity is zero and no session borrow crosses the call.
    let test = unsafe { ChangeDisplaySettingsExW(device.as_ptr(), &mut mode, 0, 2, null()) };
    log(format_args!("CASE {} CDS_TEST={}\r\n", case, test));
    if test != 0 {
        return false;
    }
    // SAFETY: The NUL-terminated device and copied public DEVMODE stay live; private capacity is zero and no session borrow crosses the call.
    let status = unsafe { ChangeDisplaySettingsExW(device.as_ptr(), &mut mode, 0, 0, null()) };
    log(format_args!("CASE {} APPLY={}\r\n", case, status));
    if status != 0 {
        return false;
    }
    SESSION.data.borrow_mut().mode_changed = true;
    if enum_mode(&device, u32::MAX).is_none_or(|actual| !actual.matches(expected)) {
        return false;
    }
    let timer = usize::from(case) + 1;
    // SAFETY: The owned window and nonzero case ID are valid; None requests a queued WM_TIMER rather than a callback pointer.
    let actual = unsafe { SetTimer(window, timer, 90000, None) };
    SESSION.data.borrow_mut().timer = actual;
    if actual != timer {
        return false;
    }
    if !last_error(
        "SetWindowPos",
        // SAFETY: The owned GUI window is live; HWND_TOPMOST is the documented -1 handle and dimensions were checked as i32.
        unsafe { SetWindowPos(window, usize::MAX, 0, 0, width, height, 0x0040) } != 0,
    ) {
        return false;
    }
    // SAFETY: The owned window is live, and no state borrow is held while focus callbacks can re-enter.
    unsafe { SetFocus(window) };
    let mut rectangle = Rect::default();
    // SAFETY: The owned window and local writable repr(C) RECT are valid for this call.
    if unsafe { GetClientRect(window, &mut rectangle) } == 0
        || rectangle.left != 0
        || rectangle.top != 0
        || rectangle.right != width
        || rectangle.bottom != height
    {
        return false;
    }
    hide_cursor()
}
fn restore(window: usize) -> bool {
    let timer = SESSION.data.borrow().timer;
    let mut valid = true;
    if timer != 0 {
        // SAFETY: This is the recorded timer of the owned window; failure remains an observed cleanup error.
        valid &= last_error("KillTimer", unsafe { KillTimer(window, timer) } != 0);
        SESSION.data.borrow_mut().timer = 0;
    }
    if SESSION.data.borrow().cursor.owned > 0 {
        valid &= show_cursor();
    }
    valid &= SESSION.data.borrow().cursor.balanced();
    if SESSION.data.borrow().mode_changed {
        let (device, mut original) = {
            let d = SESSION.data.borrow();
            (d.device, d.original)
        };
        let status =
            // SAFETY: The NUL-terminated device and copied public DEVMODE stay live; private capacity is zero and no session borrow crosses the call.
            unsafe { ChangeDisplaySettingsExW(device.as_ptr(), &mut original, 0, 0, null()) };
        log(format_args!(
            "RESTORE status={} original={}x{}x{}\r\n",
            status, original.width, original.height, original.bits
        ));
        let restored = status == 0
            && enum_mode(&device, u32::MAX).is_some_and(|mode| {
                mode.valid()
                    && mode.position == original.position
                    && mode.orientation == original.orientation
                    && mode.flags == original.flags
                    && mode.width == original.width
                    && mode.height == original.height
                    && mode.bits == original.bits
                    && mode.frequency == original.frequency
            });
        valid &= restored;
        if restored {
            SESSION.data.borrow_mut().mode_changed = false;
        }
    }
    valid
}
fn save(case: u8, bytes: &[u8; model::TEXT_BYTES]) -> bool {
    let Some(wide) = model::case_name(case) else {
        return false;
    };
    let Ok(length) = u32::try_from(bytes.len()) else {
        return false;
    };
    // SAFETY: The fixed 12-unit filename is NUL-terminated and live; CREATE_NEW uses no security or template pointer.
    let handle = unsafe { CreateFileW(wide.as_ptr(), 0x40000000, 0, null(), 1, 0x80, 0) };
    if handle == usize::MAX {
        return last_error("CreateFileW(CREATE_NEW)", false);
    }
    let mut written = 0;
    let write = last_error(
        "WriteFile(case)",
        // SAFETY: The input bytes and writable count live through this synchronous call; no OVERLAPPED or state borrow is passed.
        unsafe { win32::WriteFile(handle, bytes.as_ptr(), length, &mut written, null_mut()) } != 0,
    ) && written == length;
    // SAFETY: The successfully created file handle is still owned and has not been closed.
    let flushed = last_error("FlushFileBuffers", unsafe { FlushFileBuffers(handle) } != 0);
    // SAFETY: This handle was successfully created, remains owned and is closed exactly once here.
    let closed = last_error("CloseHandle", unsafe { CloseHandle(handle) } != 0);
    log(format_args!(
        "SAVED case={} bytes={} write={} flush={} close={}\r\n",
        case, written, write, flushed, closed
    ));
    write && flushed && closed
}
fn paint(window: usize) {
    let Some(paint) = SESSION.engine.paint() else {
        return;
    };
    let Some((mode, width, height)) = paint
        .view
        .mode()
        .filter(|mode| mode.pixels().is_some())
        .and_then(|mode| {
            Some((
                mode,
                i32::try_from(mode.width).ok()?,
                i32::try_from(mode.height).ok()?,
            ))
        })
    else {
        SESSION.engine.painted(paint, false);
        return;
    };
    let mut record = PaintStruct::default();
    // SAFETY: Windows supplied this owned WM_PAINT window; the local writable PAINTSTRUCT remains live until EndPaint.
    let dc = unsafe { BeginPaint(window, &mut record) };
    if dc == 0 {
        SESSION.engine.painted(paint, false);
        return;
    }
    let mut valid = true;
    let pixels = core::ptr::addr_of_mut!(PIXELS).cast::<u32>();
    for y in 0..mode.height {
        for x in 0..mode.width {
            let index = (y * mode.width + x) as usize;
            if let Some(color) = model::pixel(paint.view, x, y) {
                // SAFETY: The owner thread holds the drawing latch; the validated mode bounds index below the static pixel capacity.
                unsafe { pixels.add(index).write(color) }
            } else {
                valid = false;
            }
        }
    }
    let header = BitmapHeader {
        size: 40,
        width,
        height: -height,
        planes: 1,
        bits: 32,
        compression: 0,
        image_size: mode.width * mode.height * 4,
        x_pixels: 0,
        y_pixels: 0,
        colors: 0,
        important: 0,
    };
    // SAFETY: The nonzero HDC is retained from successful BeginPaint and has not reached EndPaint.
    let dc_width = unsafe { GetDeviceCaps(dc, 8) };
    // SAFETY: The nonzero HDC is retained from successful BeginPaint and has not reached EndPaint.
    let dc_height = unsafe { GetDeviceCaps(dc, 10) };
    // SAFETY: The nonzero HDC is retained from successful BeginPaint and has not reached EndPaint.
    let bits = unsafe { GetDeviceCaps(dc, 12) };
    // SAFETY: The nonzero HDC is retained from successful BeginPaint and has not reached EndPaint.
    let planes = unsafe { GetDeviceCaps(dc, 14) };
    // SAFETY: The drawing latch excludes nested writes; the static pixels cover all requested rows and the live BI_RGB header needs no colour table.
    let lines = unsafe {
        SetDIBitsToDevice(
            dc,
            0,
            0,
            mode.width,
            mode.height,
            0,
            0,
            0,
            mode.height,
            pixels.cast(),
            &header,
            0,
        )
    };
    valid &=
        dc_width == width && dc_height == height && bits == 32 && planes == 1 && lines == height;
    // SAFETY: This pointer-free call flushes the current thread GDI batch while the drawing latch remains held.
    valid &= unsafe { GdiFlush() } != 0;
    log(format_args!(
        "PAINT case={} phase={:?} geometry={}x{} DC={}x{}x{} planes={} lines={} marker={:08x}\r\n",
        paint.view.case,
        paint.view.phase,
        mode.width,
        mode.height,
        dc_width,
        dc_height,
        bits,
        planes,
        lines,
        paint.view.word()
    ));
    // SAFETY: This pairs the successful BeginPaint for the same owned window and unchanged local PAINTSTRUCT.
    valid &= unsafe { EndPaint(window, &record) } != 0;
    SESSION.engine.painted(paint, valid);
}
unsafe extern "system" fn window_proc(
    window: usize,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    owner();
    match message {
        0x000f => paint(window),
        0x0010 => SESSION.engine.abort(Error::Abort),
        // SAFETY: This pointer-free call posts the current owner thread quit code.
        0x0002 => unsafe { PostQuitMessage(0) },
        0x0100 | 0x0101 => {
            if let Ok(key) = u32::try_from(wparam) {
                SESSION.engine.key(
                    key,
                    message == 0x0100,
                    lparam.cast_unsigned() & (1 << 30) != 0,
                );
            } else {
                SESSION.engine.abort(Error::EarlyInput);
            }
        }
        0x0102 => {
            // SAFETY: VK_CAPITAL is a documented virtual key; no pointer or state borrow is passed.
            let caps = unsafe { GetKeyState(0x14) } & 1 != 0;
            let before = SESSION.engine.view();
            let bytes = lparam.to_le_bytes();
            let repeat = u16::from_le_bytes([bytes[0], bytes[1]]);
            if let Ok(character) = u32::try_from(wparam) {
                SESSION.engine.character(character, caps, repeat);
            } else {
                SESSION.engine.abort(Error::Text);
            }
            let after = SESSION.engine.view();
            log(format_args!(
                "CHAR case={} value={} caps={} repeat={} before={} after={} error={:?}\r\n",
                before.case, wparam, caps, repeat, before.text, after.text, after.error
            ));
        }
        0x0200 => {
            if let Some(position) = point() {
                let before = SESSION.engine.view();
                SESSION.engine.pointer(position);
                let after = SESSION.engine.view();
                if before.phase == Phase::Pointer {
                    log(format_args!(
                        "POINTER case={} x={} y={} accepted={} next={}\r\n",
                        before.case,
                        position.x,
                        position.y,
                        after.pointers > before.pointers,
                        after.pointers
                    ));
                }
            } else {
                SESSION.engine.abort(Error::Cursor)
            }
        }
        0x0113 => SESSION.engine.timer(wparam),
        // SAFETY: The unchanged message arguments came from Windows for this class window; no state borrow crosses default processing.
        _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
    0
}
fn drive(window: usize) {
    for _ in 0..8 {
        let Some(action) = SESSION.engine.action() else {
            break;
        };
        let case = SESSION.engine.view().case;
        let mut baseline = Point::default();
        let success = match action {
            Action::Apply(_) => apply(window, case),
            Action::Show(_) => {
                let visible = show_cursor();
                if let Some(position) = point() {
                    baseline = position;
                    log(format_args!(
                        "POINTER_BASE case={} x={} y={}\r\n",
                        case, position.x, position.y
                    ));
                    visible
                } else {
                    false
                }
            }
            Action::Save(_, bytes) => save(case, &bytes),
            Action::Restore(_, _) => restore(window),
        };
        SESSION.engine.complete(action, success, baseline);
        let view = SESSION.engine.view();
        log(format_args!(
            "PHASE case={} state={:?} pointer={} text={} error={:?} cleanup={} tick={}\r\n",
            view.case,
            view.phase,
            view.pointers,
            view.text,
            view.error,
            view.cleanup_error,
            // SAFETY: This pointer-free API reads the system tick count and does not access Rust state.
            unsafe { GetTickCount() }
        ));
    }
    if SESSION.engine.repaint()
        && matches!(
            SESSION.engine.view().phase,
            Phase::Drawing | Phase::Pixels | Phase::Pointer | Phase::Text | Phase::Saved
        )
    {
        // SAFETY: The owned window is live; NULL selects the whole client area and no state borrow crosses the call.
        if unsafe { InvalidateRect(window, null(), 0) } == 0 {
            SESSION.engine.abort(Error::Paint);
        }
        // SAFETY: The owned window is live; borrows ended and the drawing latch guards any nested paint callbacks.
        if unsafe { UpdateWindow(window) } == 0 {
            SESSION.engine.abort(Error::Paint);
        }
    }
}
fn run() -> u32 {
    SESSION
        .owner
        // SAFETY: The API takes no pointers; its thread ID is read before any private GUI state access.
        .store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    log(format_args!(
        "MODE392 begin case_count={} static_pixel_bytes={}\r\n",
        model::CASES,
        model::MAX_PIXELS * 4
    ));
    if !discover() {
        log(format_args!("DISCOVERY FAILED\r\n"));
        return 2;
    }
    let class: &[u16] = &[70, 67, 77, 111, 100, 101, 51, 57, 50, 0];
    // SAFETY: NULL requests the current process module without any borrowed name buffer.
    let instance = unsafe { GetModuleHandleW(null()) };
    // SAFETY: The low integer pointer is the documented IDC_ARROW resource ID, not a Rust memory address.
    let cursor = unsafe { LoadCursorW(0, 32512usize as *const u16) };
    let definition = WindowClass {
        style: 3,
        procedure: window_proc,
        class_extra: 0,
        window_extra: 0,
        instance,
        icon: 0,
        cursor,
        background: 0,
        menu: null(),
        name: class.as_ptr(),
    };
    // SAFETY: The class, NUL-terminated name and system-ABI callback stay valid; its module and stock cursor are nonzero.
    if instance == 0 || cursor == 0 || unsafe { RegisterClassW(&definition) } == 0 {
        return 3;
    }
    // SAFETY: The registered class and title strings remain live; the callback owner is established before re-entry and lpParam is NULL.
    let window = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0x80000000,
            0,
            0,
            640,
            480,
            0,
            0,
            instance,
            null(),
        )
    };
    if window == 0 {
        return 4;
    }
    // SAFETY: The successfully created window belongs to this owner thread; no state borrow crosses visibility callbacks.
    unsafe { ShowWindow(window, 5) };
    SESSION.engine.begin();
    loop {
        drive(window);
        let view = SESSION.engine.view();
        if matches!(view.phase, Phase::Done | Phase::Failed) {
            let result = if view.phase == Phase::Done { 0 } else { 5 };
            log(format_args!(
                "MODE392 FINISHED status={} error={:?} cleanup={}\r\n",
                result, view.error, view.cleanup_error
            ));
            // SAFETY: The window is still owned by this thread; mode and cursor cleanup has completed or its failures are retained.
            if unsafe { DestroyWindow(window) } == 0 {
                return 6;
            }
            return if SESSION.engine.view().error.is_none() {
                result
            } else {
                5
            };
        }
        // A pure phase transition can request another draw without waiting for input.
        if SESSION.engine.needs_drive()
            || (SESSION.engine.repaint()
                && matches!(SESSION.engine.view().phase, Phase::Drawing | Phase::Pixels))
        {
            continue;
        }
        let mut message = Message::default();
        // SAFETY: The local writable MSG has the target-asserted layout; no borrowed session state crosses the blocking call.
        let received = unsafe { GetMessageW(&mut message, 0, 0, 0) };
        if received <= 0 {
            SESSION.engine.abort(Error::Abort);
            let restored = restore(window);
            log(format_args!(
                "MESSAGE_LOOP exit={} restored={}\r\n",
                received, restored
            ));
            return 7;
        }
        // SAFETY: The initialized MSG came from GetMessageW; it stays live for translation and dispatch after all state borrows end.
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message)
        };
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let status = run();
    // SAFETY: The DWORD exit code is valid; process termination is intentional and is not a restore claim.
    unsafe { win32::ExitProcess(status) }
}
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: The DWORD exit code is valid; process termination is intentional and is not a restore claim.
    unsafe { win32::ExitProcess(250) }
}
