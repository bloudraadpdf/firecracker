// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use core::cell::RefCell;

pub const CASES: usize = 18;
pub const MAX_PIXELS: usize = 2560 * 1440;
pub const TEXT_BYTES: usize = 95;
const WIDTHS: [u16; CASES] = [
    640, 800, 1024, 1280, 1600, 1152, 1280, 1280, 1280, 1440, 1400, 1680, 1920, 1280, 1920, 1600,
    1366, 2560,
];
const HEIGHTS: [u16; CASES] = [
    480, 600, 768, 1024, 1200, 864, 768, 800, 960, 900, 1050, 1050, 1200, 720, 1080, 900, 768, 1440,
];

pub fn case_name(case: u8) -> Option<[u16; 12]> {
    if usize::from(case) >= CASES {
        return None;
    }
    let mut name = *b"mode-00.txt\0";
    name[5] = b'0' + case / 10;
    name[6] = b'0' + case % 10;
    Some(name.map(u16::from))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
}
impl Mode {
    pub fn at(index: usize) -> Option<Self> {
        Some(Self {
            width: u32::from(*WIDTHS.get(index)?),
            height: u32::from(*HEIGHTS.get(index)?),
        })
    }
    pub fn pixels(self) -> Option<usize> {
        let count = usize::try_from(self.width.checked_mul(self.height)?).ok()?;
        (self.width > 0 && self.height > 0 && count <= MAX_PIXELS).then_some(count)
    }
    pub fn point(self, index: u8) -> Option<Point> {
        self.pixels()?;
        let (x, y) = match index {
            0 => (0, 0),
            1 => (self.width - 1, 0),
            2 => (self.width - 1, self.height - 1),
            3 => (0, self.height - 1),
            4 => (self.width / 2, self.height / 2),
            _ => return None,
        };
        Some(Point {
            x: i32::try_from(x).ok()?,
            y: i32::try_from(y).ok()?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Boot,
    Applying,
    Drawing,
    Pixels,
    Showing,
    Pointer,
    Text,
    Saving,
    Saved,
    Restoring,
    Done,
    Failed,
}
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Abort = 1,
    Timeout,
    Apply,
    Paint,
    Cursor,
    PointerFlood,
    Text,
    Save,
    Restore,
    EarlyInput,
    Log,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    case: u8,
    epoch: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Apply(Token),
    Show(Token),
    Save(Token, [u8; TEXT_BYTES]),
    Restore(Token, bool),
}
impl Action {
    fn token(self) -> Token {
        match self {
            Self::Apply(t) | Self::Show(t) | Self::Save(t, _) | Self::Restore(t, _) => t,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub case: u8,
    pub phase: Phase,
    pub pointers: u8,
    pub text: u8,
    pub error: Option<Error>,
    pub cleanup_error: bool,
}
impl View {
    pub fn word(self) -> u32 {
        (u32::from(self.case) << 24)
            | ((self.phase as u32) << 16)
            | (u32::from(self.pointers) << 8)
            | u32::from(self.text)
    }
    pub fn mode(self) -> Option<Mode> {
        Mode::at(self.case.into())
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Paint {
    pub token: Token,
    pub view: View,
}

#[derive(Debug)]
struct Machine {
    case: u8,
    phase: Phase,
    epoch: u16,
    busy: bool,
    drawing: bool,
    repaint: bool,
    f1: bool,
    enter: bool,
    pointers: u8,
    moves: u8,
    last: Point,
    text: [u8; TEXT_BYTES],
    length: u8,
    error: Option<Error>,
    cleanup_error: bool,
    advance: bool,
}
impl Machine {
    const fn new() -> Self {
        Self {
            case: 0,
            phase: Phase::Boot,
            epoch: 0,
            busy: false,
            drawing: false,
            repaint: false,
            f1: false,
            enter: false,
            pointers: 0,
            moves: 0,
            last: Point { x: 0, y: 0 },
            text: [0; TEXT_BYTES],
            length: 0,
            error: None,
            cleanup_error: false,
            advance: false,
        }
    }
    fn view(&self) -> View {
        View {
            case: self.case,
            phase: self.phase,
            pointers: self.pointers,
            text: self.length,
            error: self.error,
            cleanup_error: self.cleanup_error,
        }
    }
    fn token(&self) -> Token {
        Token {
            case: self.case,
            epoch: self.epoch,
        }
    }
    fn phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.epoch += 1;
        self.repaint = true;
        self.f1 = false;
        self.enter = false;
    }
    fn abort(&mut self, error: Error) {
        if self.error.is_none() {
            self.error = Some(error);
        }
        if self.phase == Phase::Done {
            self.phase(Phase::Failed);
        } else if !matches!(self.phase, Phase::Restoring | Phase::Failed) {
            self.advance = false;
            self.phase(Phase::Restoring);
        }
    }
    fn action(&mut self) -> Option<Action> {
        if self.busy || self.drawing {
            return None;
        }
        let t = self.token();
        let action = match self.phase {
            Phase::Applying => Action::Apply(t),
            Phase::Showing => Action::Show(t),
            Phase::Saving => Action::Save(t, self.text),
            Phase::Restoring => Action::Restore(t, self.advance),
            _ => return None,
        };
        self.busy = true;
        Some(action)
    }
    fn complete(&mut self, action: Action, success: bool, baseline: Point) {
        self.busy = false;
        if action.token() != self.token() {
            return;
        }
        match action {
            Action::Apply(_) => {
                if success {
                    self.phase(Phase::Drawing)
                } else {
                    self.abort(Error::Apply)
                }
            }
            Action::Show(_) => {
                if success {
                    self.last = baseline;
                    self.phase(Phase::Pointer)
                } else {
                    self.abort(Error::Cursor)
                }
            }
            Action::Save(_, _) => {
                if success {
                    self.phase(Phase::Saved)
                } else {
                    self.abort(Error::Save)
                }
            }
            Action::Restore(_, advance) => {
                if !success {
                    self.cleanup_error = true;
                    if self.error.is_none() {
                        self.error = Some(Error::Restore);
                    }
                }
                if success && advance && self.error.is_none() {
                    if usize::from(self.case) + 1 == CASES {
                        self.phase(Phase::Done);
                    } else {
                        self.case += 1;
                        self.pointers = 0;
                        self.moves = 0;
                        self.length = 0;
                        self.text = [0; TEXT_BYTES];
                        self.advance = false;
                        self.phase(Phase::Applying);
                    }
                } else {
                    self.phase(Phase::Failed);
                }
            }
        }
    }
    fn key(&mut self, key: u32, down: bool, repeat: bool) {
        if key == 27 && down && !repeat {
            self.abort(Error::Abort);
            return;
        }
        if key == 112 {
            if down && !repeat {
                if self.phase == Phase::Pixels {
                    self.f1 = true;
                } else {
                    self.abort(Error::EarlyInput);
                }
            } else if !down && self.phase == Phase::Pixels && self.f1 {
                self.phase(Phase::Showing);
            }
        } else if key == 13 {
            if down && !repeat {
                if self.phase == Phase::Saved {
                    self.enter = true;
                } else {
                    self.abort(Error::EarlyInput);
                }
            } else if !down && self.phase == Phase::Saved && self.enter {
                self.advance = true;
                self.phase(Phase::Restoring);
            }
        }
    }
    fn character(&mut self, value: u32, caps: bool, repeat: u16) {
        if value == 13 && self.phase == Phase::Saved || value == 27 {
            return;
        }
        if self.phase != Phase::Text
            || repeat != 1
            || !caps
            || self.length as usize >= TEXT_BYTES
            || value != 32 + u32::from(self.length)
        {
            self.abort(Error::Text);
            return;
        }
        let Ok(byte) = u8::try_from(value) else {
            self.abort(Error::Text);
            return;
        };
        self.text[usize::from(self.length)] = byte;
        self.length += 1;
        self.repaint = true;
        if self.length as usize == TEXT_BYTES {
            self.phase(Phase::Saving);
        }
    }
    fn pointer(&mut self, point: Point) {
        if self.phase != Phase::Pointer {
            return;
        }
        self.moves += 1;
        if self.moves > 64 {
            self.abort(Error::PointerFlood);
            return;
        }
        let previous = self.last;
        self.last = point;
        if point != previous
            && Mode::at(self.case.into()).and_then(|m| m.point(self.pointers)) == Some(point)
        {
            self.pointers += 1;
            self.repaint = true;
            if self.pointers == 5 {
                self.phase(Phase::Text);
            }
        }
    }
    fn paint(&mut self) -> Option<Paint> {
        if self.drawing {
            self.repaint = true;
            return None;
        }
        if matches!(
            self.phase,
            Phase::Boot | Phase::Applying | Phase::Restoring | Phase::Done | Phase::Failed
        ) {
            return None;
        }
        self.drawing = true;
        self.repaint = false;
        Some(Paint {
            token: self.token(),
            view: self.view(),
        })
    }
    fn painted(&mut self, paint: Paint, ok: bool) {
        self.drawing = false;
        if !ok {
            self.abort(Error::Paint);
            return;
        }
        if paint.token == self.token() && self.phase == Phase::Drawing {
            self.phase(Phase::Pixels);
        }
    }
}

#[derive(Debug)]
pub struct Engine(RefCell<Machine>);
impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}
impl Engine {
    pub const fn new() -> Self {
        Self(RefCell::new(Machine::new()))
    }
    pub fn view(&self) -> View {
        self.0.borrow().view()
    }
    pub fn begin(&self) {
        let mut s = self.0.borrow_mut();
        if s.phase == Phase::Boot {
            s.phase(Phase::Applying);
        }
    }
    pub fn action(&self) -> Option<Action> {
        self.0.borrow_mut().action()
    }
    pub fn complete(&self, a: Action, ok: bool, baseline: Point) {
        self.0.borrow_mut().complete(a, ok, baseline);
    }
    pub fn abort(&self, e: Error) {
        self.0.borrow_mut().abort(e);
    }
    pub fn key(&self, key: u32, down: bool, repeat: bool) {
        self.0.borrow_mut().key(key, down, repeat);
    }
    pub fn character(&self, c: u32, caps: bool, repeat: u16) {
        self.0.borrow_mut().character(c, caps, repeat);
    }
    pub fn pointer(&self, p: Point) {
        self.0.borrow_mut().pointer(p);
    }
    pub fn timer(&self, id: usize) {
        let mut s = self.0.borrow_mut();
        if id == usize::from(s.case) + 1 {
            s.abort(Error::Timeout);
        }
    }
    pub fn paint(&self) -> Option<Paint> {
        self.0.borrow_mut().paint()
    }
    pub fn painted(&self, p: Paint, ok: bool) {
        self.0.borrow_mut().painted(p, ok);
    }
    pub fn needs_drive(&self) -> bool {
        let s = self.0.borrow();
        !s.busy
            && !s.drawing
            && matches!(
                s.phase,
                Phase::Applying | Phase::Showing | Phase::Saving | Phase::Restoring
            )
    }
    pub fn repaint(&self) -> bool {
        let s = self.0.borrow();
        s.repaint && !s.drawing
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CursorBalance {
    pub owned: u8,
    initial: Option<i32>,
    last: Option<i32>,
}
impl CursorBalance {
    pub const fn new() -> Self {
        Self {
            owned: 0,
            initial: None,
            last: None,
        }
    }
    pub fn hide(&mut self, value: i32) -> bool {
        if self.owned >= 16 {
            return false;
        }
        self.owned += 1;
        if self.initial.is_none() {
            self.initial = value.checked_add(1);
        }
        let Some(initial) = self.initial else {
            return false;
        };
        let expected = initial.checked_sub(i32::from(self.owned));
        self.last = Some(value);
        initial >= 0 && expected == Some(value) && value < 0
    }
    pub fn can_hide_more(self) -> bool {
        self.owned < 16 && self.initial.is_some_and(|n| n >= 0) && self.last.is_some_and(|n| n >= 0)
    }
    pub fn restored(&mut self, value: i32) -> bool {
        if self.owned == 0 {
            return false;
        }
        self.owned -= 1;
        let expected = self.last.and_then(|n| n.checked_add(1));
        self.last = Some(value);
        expected == Some(value)
    }
    pub fn balanced(self) -> bool {
        self.owned == 0 && self.initial == self.last && self.last.is_none_or(|n| n >= 0)
    }
}

pub fn pixel(view: View, x: u32, y: u32) -> Option<u32> {
    let m = view.mode()?;
    if x >= m.width || y >= m.height {
        return None;
    }
    let color = if x == 0 && y == 0 {
        0xffffff
    } else if x == m.width - 1 && y == 0 {
        0xff00ff
    } else if x == 0 && y == m.height - 1 {
        0x00ffff
    } else if x == m.width - 1 && y == m.height - 1 {
        0x000000
    } else if y == 0 {
        0x0000ff
    } else if y == m.height - 1 {
        0xffff00
    } else if x == 0 {
        0xff0000
    } else if x == m.width - 1 {
        0x00ff00
    } else if (16..80).contains(&x) && (16..48).contains(&y) {
        let bit = ((y - 16) / 8) * 8 + (x - 16) / 8;
        if view.word() & (1 << bit) != 0 {
            0xffffff
        } else {
            0x202020
        }
    } else {
        (((x * 17 + u32::from(view.case) * 29) & 255) << 16)
            | (((y * 31 + u32::from(view.case) * 13) & 255) << 8)
            | (((x ^ (y * 3)) + u32::from(view.case) * 7) & 255)
    };
    Some(color)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pixels(e: &Engine) {
        e.begin();
        let a = e.action().unwrap();
        e.complete(a, true, Point::default());
        let p = e.paint().unwrap();
        e.painted(p, true);
        assert_eq!(e.view().phase, Phase::Pixels);
    }
    fn pointer(e: &Engine, baseline: Point) {
        pixels(e);
        e.key(112, true, false);
        e.key(112, false, false);
        let a = e.action().unwrap();
        e.complete(a, true, baseline);
    }
    fn text(e: &Engine) {
        pointer(e, Point { x: 1, y: 1 });
        for i in 0..5 {
            e.pointer(Mode::at(0).unwrap().point(i).unwrap());
        }
        assert_eq!(e.view().phase, Phase::Text);
    }
    #[test]
    fn case_files_are_distinct_fixed_relative_labels() {
        for case in 0..u8::try_from(CASES).unwrap() {
            let name = case_name(case).unwrap();
            assert_eq!(name[11], 0);
            assert_eq!(&name[..5], &[109, 111, 100, 101, 45]);
            assert_eq!(&name[7..11], &[46, 116, 120, 116]);
            for previous in 0..case {
                assert_ne!(Some(name), case_name(previous));
            }
        }
        assert_eq!(case_name(u8::try_from(CASES).unwrap()), None);
        assert_eq!(case_name(u8::MAX), None);
    }
    #[test]
    fn modes_and_storage_have_exact_limits() {
        assert_eq!(MAX_PIXELS * 4, 14_745_600);
        let mut largest = 0;
        for i in 0..CASES {
            let m = Mode::at(i).unwrap();
            largest = largest.max(m.pixels().unwrap());
            assert!(m.width <= 2560 && m.height <= 1440);
        }
        assert_eq!(largest, MAX_PIXELS);
        assert_eq!(Mode::at(CASES), None);
        assert!(
            Mode {
                width: u32::MAX,
                height: 2
            }
            .pixels()
            .is_none()
        );
    }
    #[test]
    fn pixel_oracle_covers_edges_rows_and_phase_marker() {
        let e = Engine::new();
        pixels(&e);
        let v = e.view();
        assert_eq!(pixel(v, 0, 0), Some(0xffffff));
        assert_eq!(pixel(v, 639, 479), Some(0));
        assert_eq!(pixel(v, 1, 1), Some(0x111f02));
        assert_eq!(pixel(v, 1, 2), Some(0x113e07));
        assert_eq!(pixel(v, 640, 0), None);
        assert_ne!(
            v.word(),
            View {
                phase: Phase::Pointer,
                ..v
            }
            .word()
        );
    }
    #[test]
    fn pixel_confirmation_requires_fresh_f1_release() {
        let e = Engine::new();
        pixels(&e);
        e.key(112, false, false);
        assert_eq!(e.view().phase, Phase::Pixels);
        e.key(112, true, false);
        e.key(112, true, true);
        e.key(112, false, false);
        assert_eq!(e.view().phase, Phase::Showing);
        e.key(112, false, false);
        assert_eq!(e.view().phase, Phase::Showing);
    }
    #[test]
    fn baseline_corner_needs_prime_away_and_back() {
        let e = Engine::new();
        pointer(&e, Point::default());
        e.pointer(Point::default());
        assert_eq!(e.view().pointers, 0);
        e.pointer(Point { x: 3, y: 3 });
        assert_eq!(e.view().pointers, 0);
        e.pointer(Point::default());
        assert_eq!(e.view().pointers, 1);
    }
    #[test]
    fn pointer_intermediates_are_bounded() {
        let e = Engine::new();
        pointer(&e, Point::default());
        for _ in 0..65 {
            e.pointer(Point { x: 5, y: 5 });
        }
        assert_eq!(e.view().error, Some(Error::PointerFlood));
    }
    #[test]
    fn space_is_data_and_enter_never_appends_newline() {
        let e = Engine::new();
        text(&e);
        e.key(32, true, false);
        e.character(32, true, 1);
        e.key(32, false, false);
        assert_eq!(e.view().text, 1);
        for c in 33..=126 {
            e.character(c, true, 1);
        }
        let a = e.action().unwrap();
        let expected: [u8; TEXT_BYTES] = core::array::from_fn(|i| u8::try_from(32 + i).unwrap());
        assert!(matches!(a, Action::Save(_, bytes) if bytes == expected));
        e.complete(a, true, Point::default());
        e.character(13, true, 1);
        assert_eq!(e.view().text, 95);
        e.key(13, true, false);
        e.key(13, false, false);
        assert!(matches!(e.action(), Some(Action::Restore(_, true))));
    }
    #[test]
    fn wrong_caps_or_early_enter_abort_without_advancing() {
        for caps in [false, true] {
            let e = Engine::new();
            text(&e);
            e.character(if caps { 33 } else { 32 }, caps, 1);
            assert_eq!(e.view().error, Some(Error::Text));
        }
        let e = Engine::new();
        pixels(&e);
        e.key(13, true, false);
        assert_eq!(e.view().error, Some(Error::EarlyInput));
    }
    #[test]
    fn coalesced_or_zero_character_repeat_never_counts_as_one_input() {
        for repeat in [0, 2, u16::MAX] {
            let e = Engine::new();
            text(&e);
            e.character(32, true, repeat);
            assert_eq!(e.view().text, 0);
            assert_eq!(e.view().error, Some(Error::Text));
        }
    }
    #[test]
    fn extra_character_and_save_failure_keep_original_error() {
        let e = Engine::new();
        text(&e);
        for c in 32..=126 {
            e.character(c, true, 1);
        }
        let a = e.action().unwrap();
        e.character(127, true, 1);
        e.complete(a, false, Point::default());
        assert_eq!(e.view().error, Some(Error::Text));
        let restore = e.action().unwrap();
        e.complete(restore, false, Point::default());
        assert!(e.view().cleanup_error);
        assert_eq!(e.view().error, Some(Error::Text));
    }
    #[test]
    fn reentry_during_mock_ffi_has_no_borrow_and_stale_commit() {
        let e = Engine::new();
        e.begin();
        let a = e.action().unwrap();
        let mock_ffi = || {
            e.abort(Error::Abort);
            assert!(e.action().is_none());
        };
        mock_ffi();
        e.complete(a, true, Point::default());
        assert_eq!(e.view().phase, Phase::Restoring);
        assert_eq!(e.view().error, Some(Error::Abort));
    }
    #[test]
    fn recursive_paint_cannot_acquire_static_storage() {
        let e = Engine::new();
        pixels(&e);
        let p = e.paint().unwrap();
        assert!(e.paint().is_none());
        e.abort(Error::Abort);
        e.painted(p, true);
        assert_eq!(e.view().phase, Phase::Restoring);
    }
    #[test]
    fn failed_paint_requires_cleanup_before_the_next_wait() {
        let e = Engine::new();
        pixels(&e);
        let paint = e.paint().unwrap();
        assert!(!e.needs_drive());
        e.painted(paint, false);
        assert!(e.needs_drive());
        assert!(matches!(e.action(), Some(Action::Restore(_, false))));
    }
    #[test]
    fn timer_ids_and_failed_show_are_truthful() {
        let e = Engine::new();
        pixels(&e);
        e.timer(2);
        assert_eq!(e.view().phase, Phase::Pixels);
        e.key(112, true, false);
        e.key(112, false, false);
        let a = e.action().unwrap();
        e.complete(a, false, Point::default());
        assert_eq!(e.view().error, Some(Error::Cursor));
        e.timer(1);
        assert_eq!(e.view().error, Some(Error::Cursor));
    }
    #[test]
    fn final_output_failure_cannot_leave_a_successful_terminal_phase() {
        let mut machine = Machine::new();
        machine.phase(Phase::Done);
        machine.abort(Error::Log);
        assert_eq!(machine.phase, Phase::Failed);
        assert_eq!(machine.error, Some(Error::Log));
        assert!(machine.action().is_none());
    }
    #[test]
    fn cursor_balance_restores_exact_owned_count() {
        for original in 0..16 {
            let mut b = CursorBalance::default();
            let mut value = original;
            loop {
                value -= 1;
                if b.hide(value) {
                    break;
                }
                assert!(b.can_hide_more());
            }
            assert_eq!(b.owned, u8::try_from(original).unwrap() + 1);
            while b.owned > 0 {
                value += 1;
                assert!(b.restored(value));
            }
            assert!(b.balanced());
        }
    }
    #[test]
    fn cursor_hidden_changed_and_excess_counts_do_not_force_pass() {
        let mut b = CursorBalance::default();
        assert!(!b.hide(-2));
        assert!(!b.can_hide_more());
        assert!(b.restored(-1));
        assert!(!b.balanced());
        let mut b = CursorBalance::default();
        for value in (0..16).rev() {
            assert!(!b.hide(value));
        }
        assert!(!b.can_hide_more());
        let mut b = CursorBalance::default();
        assert!(b.hide(-1));
        assert!(!b.restored(1));
        assert!(!b.balanced());
    }
    #[test]
    fn sixteen_cases_end_only_after_successful_restore() {
        let e = Engine::new();
        for case in 0..CASES {
            if case == 0 {
                pixels(&e);
            } else {
                let a = e.action().unwrap();
                e.complete(a, true, Point::default());
                let p = e.paint().unwrap();
                e.painted(p, true);
            }
            assert_eq!(e.view().case as usize, case);
            e.key(112, true, false);
            e.key(112, false, false);
            let a = e.action().unwrap();
            e.complete(a, true, Point { x: 1, y: 1 });
            for i in 0..5 {
                e.pointer(Mode::at(case).unwrap().point(i).unwrap());
            }
            for c in 32..=126 {
                e.character(c, true, 1);
            }
            let a = e.action().unwrap();
            e.complete(a, true, Point::default());
            e.key(13, true, false);
            e.key(13, false, false);
            let a = e.action().unwrap();
            e.complete(a, true, Point::default());
        }
        assert_eq!(e.view().phase, Phase::Done);
        assert!(e.action().is_none());
    }
}
