//! Finding the host's pointer in the decoded picture.
//!
//! A computer shown through a connector cannot say when its pointer is drawn
//! into its picture (Windows does that while a window is moved). The host
//! only says "the pointer may be in the picture" (`pointer_maybe_in_picture`
//! of `host_cursor`) while the viewer's left button is held and dragged; the
//! viewer then looks for the host's cursor image in the decoded picture at
//! the positions it sent itself. Found on three consecutive checks: the
//! viewer's own pointer is hidden for the rest of the drag, exactly as for
//! `in_window_move_size`. Not found (or the picture cannot be read): the
//! viewer's own pointer stays.
//!
//! The web client has the same matcher with the same numbers
//! (`web/`); change neither alone.
//!
//! Three parts: the matcher (pure functions over a cursor image, a luma
//! view and positions), the decision over one drag (`Drag`), and the state
//! shared between the threads (`Tracker` behind the functions at the end):
//! the UI thread notes the button and the positions sent, the data channel
//! notes the cursor image and "maybe", the decode thread asks whether a
//! frame is wanted and hands over the luma around the positions, a worker
//! thread runs the check.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A cursor pixel with at least this alpha is ink.
pub const INK_ALPHA: u8 = 128;
/// A cursor with fewer ink pixels cannot be told from a background.
pub const MIN_INK_PIXELS: usize = 40;
/// Nor can one whose ink pixels differ less than this in luma (all white).
pub const MIN_LUMA_SPREAD: u8 = 96;
/// The sizes the host may draw its cursor at, in the order tried.
pub const SCALES: [f32; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];
/// A template is tried this many pixels around each position, each way.
pub const SEARCH: i32 = 3;
/// A placement matches when the mean absolute luma difference over the ink
/// pixels is at most this.
pub const MAX_SCORE: u32 = 26;
/// Found on this many consecutive checks = in the picture.
pub const CHECKS_NEEDED: u32 = 3;
/// At most one check in this time.
pub const CHECK_INTERVAL: Duration = Duration::from_millis(30);
/// After this many misses in a row the checks slow down to
/// `SLOW_CHECK_INTERVAL`: a drag that is not a window's (selecting an area)
/// must not read the picture all along (web/src/pointerInPicture.ts: there
/// each check copies a frame back from the GPU and the stream skips frames).
pub const FAST_MISSES: u32 = 10;
pub const SLOW_CHECK_INTERVAL: Duration = Duration::from_millis(300);
/// Positions sent longer ago are not looked at (the newest one always is).
pub const POSITION_WINDOW: Duration = Duration::from_millis(600);
pub const MAX_POSITIONS: usize = 16;

/// The host's cursor image as `host_cursor` delivers it: RGBA, 8 bit,
/// straight alpha.
#[derive(Clone, PartialEq, Eq)]
pub struct CursorImage {
    pub width: i32,
    pub height: i32,
    pub hot_x: i32,
    pub hot_y: i32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy)]
struct Ink {
    x: u16,
    y: u16,
    luma: u8,
}

/// The cursor at one scale: its ink pixels with the luma they should have
/// in the picture.
pub struct Template {
    pub scale: f32,
    pub width: i32,
    pub height: i32,
    pub hot_x: i32,
    pub hot_y: i32,
    ink: Vec<Ink>,
}

impl Template {
    pub fn ink_pixels(&self) -> usize {
        self.ink.len()
    }
}

/// The luma of a cursor pixel. `bt709` = the picture's luma was made with
/// the Rec. 709 weights (the decoded Y plane of such a stream); otherwise
/// the Rec. 601 weights of the rule (0.299, 0.587, 0.114). The two only
/// differ for coloured pixels.
fn pixel_luma(r: u8, g: u8, b: u8, bt709: bool) -> u8 {
    let (wr, wg, wb) = if bt709 { (0.2126, 0.7152, 0.0722) } else { (0.299, 0.587, 0.114) };
    (wr * r as f64 + wg * g as f64 + wb * b as f64).round().clamp(0.0, 255.0) as u8
}

/// Rounds half up, as JavaScript's `Math.round` does (the web client).
fn round_half_up(value: f64) -> i32 {
    (value + 0.5).floor() as i32
}

fn template_at(cursor: &CursorImage, scale: f32, bt709: bool) -> Template {
    let s = scale as f64;
    let width = round_half_up(cursor.width as f64 * s).max(1);
    let height = round_half_up(cursor.height as f64 * s).max(1);
    let mut ink = Vec::new();
    for y in 0..height {
        // Nearest neighbour: the source pixel under the centre of the
        // destination pixel.
        let sy = (((y as f64 + 0.5) * cursor.height as f64 / height as f64).floor() as i32)
            .clamp(0, cursor.height - 1);
        for x in 0..width {
            let sx = (((x as f64 + 0.5) * cursor.width as f64 / width as f64).floor() as i32)
                .clamp(0, cursor.width - 1);
            let at = (sy as usize * cursor.width as usize + sx as usize) * 4;
            let px = &cursor.rgba[at..at + 4];
            if px[3] >= INK_ALPHA {
                ink.push(Ink { x: x as u16, y: y as u16, luma: pixel_luma(px[0], px[1], px[2], bt709) });
            }
        }
    }
    Template {
        scale,
        width,
        height,
        hot_x: round_half_up(cursor.hot_x as f64 * s),
        hot_y: round_half_up(cursor.hot_y as f64 * s),
        ink,
    }
}

/// The templates of a cursor, one per scale in the order tried; none when
/// the cursor cannot be told from a background (too few ink pixels, or all
/// of one brightness) or is not a valid image.
pub fn templates(cursor: &CursorImage, bt709: bool) -> Vec<Template> {
    if cursor.width <= 0
        || cursor.height <= 0
        || cursor.width > 256
        || cursor.height > 256
        || cursor.rgba.len() != cursor.width as usize * cursor.height as usize * 4
    {
        return Vec::new();
    }
    let base = template_at(cursor, 1.0, bt709);
    let min = base.ink.iter().map(|p| p.luma).min().unwrap_or(0);
    let max = base.ink.iter().map(|p| p.luma).max().unwrap_or(0);
    if base.ink.len() < MIN_INK_PIXELS || max - min < MIN_LUMA_SPREAD {
        return Vec::new();
    }
    SCALES.iter().map(|&scale| template_at(cursor, scale, bt709)).collect()
}

/// Luma (0..255, full range) of a part of the picture: `width` x `height`
/// pixels whose top left is the stream pixel (`x0`, `y0`). Outside it there
/// is no value.
pub struct LumaView<'a> {
    pub data: &'a [u8],
    pub stride: usize,
    pub width: i32,
    pub height: i32,
    pub x0: i32,
    pub y0: i32,
}

/// The sum of |luma - template luma| over the ink pixels of the template
/// with its top left at stream pixel (`left`, `top`); `None` when an ink
/// pixel has no luma (outside the picture) or the sum passes `limit`.
fn placement_sum(template: &Template, luma: &LumaView, left: i32, top: i32, limit: u32) -> Option<u32> {
    let lx = left - luma.x0;
    let ly = top - luma.y0;
    let inside = lx >= 0 && ly >= 0 && lx + template.width <= luma.width && ly + template.height <= luma.height;
    let mut sum = 0u32;
    for ink in &template.ink {
        let x = lx + ink.x as i32;
        let y = ly + ink.y as i32;
        if !inside && (x < 0 || y < 0 || x >= luma.width || y >= luma.height) {
            return None;
        }
        let value = luma.data[y as usize * luma.stride + x as usize];
        sum += (value as i32 - ink.luma as i32).unsigned_abs();
        if sum > limit {
            return None;
        }
    }
    Some(sum)
}

/// The score of one placement: the mean absolute luma difference over the
/// ink pixels; `None` when an ink pixel is outside the picture.
pub fn score(template: &Template, luma: &LumaView, left: i32, top: i32) -> Option<f32> {
    if template.ink.is_empty() {
        return None;
    }
    placement_sum(template, luma, left, top, u32::MAX).map(|sum| sum as f32 / template.ink.len() as f32)
}

/// One check: is the cursor in the picture at one of the positions (stream
/// pixels of the hot spot, newest first)? Answers the scale it was found at.
pub fn check(templates: &[Template], luma: &LumaView, positions: &[(i32, i32)]) -> Option<f32> {
    for &(px, py) in positions {
        for template in templates {
            if template.ink.is_empty() {
                continue;
            }
            let limit = MAX_SCORE * template.ink.len() as u32;
            for dy in -SEARCH..=SEARCH {
                for dx in -SEARCH..=SEARCH {
                    let left = px - template.hot_x + dx;
                    let top = py - template.hot_y + dy;
                    if placement_sum(template, luma, left, top, limit).is_some() {
                        return Some(template.scale);
                    }
                }
            }
        }
    }
    None
}

/// The decision over drags: a drag is on while the left button is held and
/// the host says the pointer may be in the picture, or the host is a
/// Windows computer.
#[derive(Default)]
pub struct Drag {
    left_held: bool,
    maybe: bool,
    /// The host is a Windows computer: every drag looks (web:
    /// `looksForPointerInPicture`). Windows draws its pointer into the
    /// picture while a window is dragged; the host says so
    /// (`in_window_move_size`) only for the system's own move loop, not for
    /// the many apps that move their windows themselves.
    windows_host: bool,
    /// The host says app-dragged windows in `in_window_move_size` itself
    /// (`window_drag_watched`, agent window_drag.rs): its drags are not
    /// looked at.
    window_drag_watched: bool,
    active: bool,
    checks: u32,
    run: u32,
    /// Checks in a row that did not find it.
    misses: u32,
    found_after: Option<u32>,
    scale: Option<f32>,
    /// Changes whenever a drag starts or ends: a check started in one drag
    /// says nothing about the next.
    generation: u64,
}

impl Drag {
    /// The viewer's left button. Answers the log line of the drag this ends.
    pub fn set_left_held(&mut self, held: bool) -> Option<String> {
        self.left_held = held;
        self.update()
    }

    /// The host's `pointer_maybe_in_picture`. Answers the log line of the
    /// drag this ends.
    pub fn set_maybe(&mut self, maybe: bool) -> Option<String> {
        self.maybe = maybe;
        self.update()
    }

    /// The host's system, as it said it ("windows", "linux", ...).
    pub fn set_host_os(&mut self, host_os: &str) -> Option<String> {
        self.windows_host = host_os.eq_ignore_ascii_case("windows");
        self.update()
    }

    /// The host's `window_drag_watched`.
    pub fn set_window_drag_watched(&mut self, watched: bool) -> Option<String> {
        self.window_drag_watched = watched;
        self.update()
    }

    fn update(&mut self) -> Option<String> {
        let on = self.left_held && (self.maybe || (self.windows_host && !self.window_drag_watched));
        if on == self.active {
            return None;
        }
        self.active = on;
        self.generation += 1;
        let line = if on {
            None
        } else {
            Some(match self.found_after {
                Some(n) => format!("pointer looked for in the picture: found after {} checks", n),
                None => format!("pointer looked for in the picture: not found ({} checks)", self.checks),
            })
        };
        self.checks = 0;
        self.run = 0;
        self.misses = 0;
        self.found_after = None;
        self.scale = None;
        line
    }

    pub fn maybe(&self) -> bool {
        self.maybe
    }

    pub fn active(&self) -> bool {
        self.active
    }

    /// The pointer was found in the picture in this drag: hide the viewer's.
    pub fn in_picture(&self) -> bool {
        self.active && self.found_after.is_some()
    }

    /// A drag is on and undecided.
    pub fn wants_check(&self) -> bool {
        self.active && self.found_after.is_none()
    }

    /// The least time between two checks now: longer after many misses.
    pub fn check_interval(&self) -> Duration {
        if self.misses >= FAST_MISSES { SLOW_CHECK_INTERVAL } else { CHECK_INTERVAL }
    }

    /// The scale that matched in this drag: the only one still tried.
    pub fn scale(&self) -> Option<f32> {
        self.scale
    }

    /// The answer of one check (the scale it was found at).
    pub fn record_check(&mut self, found: Option<f32>) {
        if !self.wants_check() {
            return;
        }
        self.checks += 1;
        match found {
            Some(scale) => {
                self.scale.get_or_insert(scale);
                self.run += 1;
                self.misses = 0;
                if self.run >= CHECKS_NEEDED {
                    self.found_after = Some(self.checks);
                }
            }
            None => {
                self.run = 0;
                self.misses += 1;
            }
        }
    }
}

struct Sent {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    at: Instant,
}

/// What the decode thread is to read for one check, and what the check
/// then needs.
pub struct Plan {
    /// The part of the picture to read, in stream pixels.
    pub x0: i32,
    pub y0: i32,
    pub width: i32,
    pub height: i32,
    positions: Vec<(i32, i32)>,
    templates: Arc<Vec<Template>>,
    scale: Option<f32>,
    generation: u64,
}

impl Plan {
    /// Runs the check on the luma read for this plan (`width` x `height`
    /// values, full range, row after row).
    pub fn check(&self, luma: &[u8]) -> Option<f32> {
        if self.width <= 0 || self.height <= 0 || luma.len() < self.width as usize * self.height as usize {
            return None;
        }
        let view = LumaView {
            data: luma,
            stride: self.width as usize,
            width: self.width,
            height: self.height,
            x0: self.x0,
            y0: self.y0,
        };
        match self.scale {
            Some(scale) => {
                let Some(at) = self.templates.iter().position(|t| t.scale == scale) else {
                    return None;
                };
                check(&self.templates[at..at + 1], &view, &self.positions)
            }
            None => check(&self.templates, &view, &self.positions),
        }
    }
}

#[derive(Default)]
pub struct Tracker {
    cursor: Option<CursorImage>,
    cursor_version: u64,
    /// The templates of (`cursor_version`, BT.709 luma).
    templates: Option<(u64, bool, Arc<Vec<Template>>)>,
    sent: VecDeque<Sent>,
    drag: Drag,
    last_check: Option<Instant>,
    in_flight: bool,
    /// (maybe, in the picture) as last told to the UI.
    reported: (bool, bool),
}

impl Tracker {
    pub fn note_cursor(&mut self, cursor: CursorImage) {
        if self.cursor.as_ref() != Some(&cursor) {
            self.cursor = Some(cursor);
            self.cursor_version += 1;
        }
    }

    /// A pointer position the viewer sent to the host: (`x`, `y`) of a
    /// picture shown `width` x `height` large.
    pub fn note_position(&mut self, x: i32, y: i32, width: i32, height: i32, now: Instant) {
        if width <= 0 || height <= 0 {
            return;
        }
        if let Some(last) = self.sent.back_mut() {
            if (last.x, last.y, last.width, last.height) == (x, y, width, height) {
                last.at = now;
                return;
            }
        }
        while self.sent.front().is_some_and(|s| now.duration_since(s.at) > POSITION_WINDOW) && self.sent.len() > 1 {
            self.sent.pop_front();
        }
        // 600 ms of a 250 Hz pointer is 150 positions.
        if self.sent.len() >= 512 {
            self.sent.pop_front();
        }
        self.sent.push_back(Sent { x, y, width, height, at: now });
    }

    /// The positions to look at in a picture of this size, newest first.
    /// The newest one always (a pointer held still is still there), older
    /// ones while not older than `POSITION_WINDOW`. A position within
    /// `SEARCH` pixels of one already taken is a repeat: the search around
    /// that one covers it.
    pub fn positions(&self, picture_width: i32, picture_height: i32, now: Instant) -> Vec<(i32, i32)> {
        let mut out: Vec<(i32, i32)> = Vec::new();
        for sent in self.sent.iter().rev() {
            if !out.is_empty() && now.duration_since(sent.at) > POSITION_WINDOW {
                break;
            }
            let x = round_half_up(sent.x as f64 * picture_width as f64 / sent.width as f64);
            let y = round_half_up(sent.y as f64 * picture_height as f64 / sent.height as f64);
            if out.iter().any(|&(ox, oy)| (ox - x).abs() <= SEARCH && (oy - y).abs() <= SEARCH) {
                continue;
            }
            out.push((x, y));
            if out.len() >= MAX_POSITIONS {
                break;
            }
        }
        out
    }

    fn wanted(&self) -> bool {
        self.drag.wants_check() && !self.in_flight && self.cursor.is_some()
    }

    /// The decode thread has a frame of this size and luma matrix: what to
    /// read of it, if a check is due. A check that needs no picture (a
    /// cursor that cannot be told from a background, no position) is
    /// answered here: not found.
    pub fn plan(&mut self, picture_width: i32, picture_height: i32, bt709: bool, now: Instant) -> Option<Plan> {
        if !self.wanted() || picture_width <= 0 || picture_height <= 0 {
            return None;
        }
        let interval = self.drag.check_interval();
        if self.last_check.is_some_and(|t| now.duration_since(t) < interval) {
            return None;
        }
        self.last_check = Some(now);
        let cursor = self.cursor.as_ref()?;
        let fresh = matches!(&self.templates, Some((v, m, _)) if *v == self.cursor_version && *m == bt709);
        if !fresh {
            self.templates = Some((self.cursor_version, bt709, Arc::new(templates(cursor, bt709))));
        }
        let templates = Arc::clone(&self.templates.as_ref()?.2);
        let positions = self.positions(picture_width, picture_height, now);
        let scale = self.drag.scale();
        let pad = templates
            .iter()
            .filter(|t| scale.is_none_or(|s| s == t.scale))
            .map(|t| t.width.max(t.height))
            .max()
            .unwrap_or(0)
            + SEARCH;
        let left = positions.iter().map(|p| p.0).min().unwrap_or(0).saturating_sub(pad).max(0);
        let top = positions.iter().map(|p| p.1).min().unwrap_or(0).saturating_sub(pad).max(0);
        let right = positions.iter().map(|p| p.0).max().unwrap_or(0).saturating_add(pad + 1).min(picture_width);
        let bottom = positions.iter().map(|p| p.1).max().unwrap_or(0).saturating_add(pad + 1).min(picture_height);
        if templates.is_empty() || positions.is_empty() || right <= left || bottom <= top {
            self.drag.record_check(None);
            return None;
        }
        self.in_flight = true;
        Some(Plan {
            x0: left,
            y0: top,
            width: right - left,
            height: bottom - top,
            positions,
            templates,
            scale,
            generation: self.drag.generation,
        })
    }

    /// The answer of the check of `plan`.
    pub fn finish(&mut self, plan: &Plan, found: Option<f32>) {
        self.in_flight = false;
        if plan.generation == self.drag.generation {
            self.drag.record_check(found);
        }
    }

    /// The picture could not be read for `plan`: no check was made.
    pub fn abandon(&mut self, _plan: &Plan) {
        self.in_flight = false;
    }

    pub fn drag(&self) -> &Drag {
        &self.drag
    }

    pub fn drag_mut(&mut self) -> &mut Drag {
        &mut self.drag
    }
}

// ---- The one tracker of the running client ----

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);
/// A frame's luma is wanted (a drag is on and undecided, no check running).
static WANTED: AtomicBool = AtomicBool::new(false);
/// (maybe, in the picture) changed since the UI last asked.
static CHANGED: AtomicBool = AtomicBool::new(false);

fn with_tracker<R>(f: impl FnOnce(&mut Tracker) -> R) -> R {
    let mut guard = TRACKER.lock().unwrap_or_else(|e| e.into_inner());
    let tracker = guard.get_or_insert_with(Tracker::default);
    let result = f(tracker);
    WANTED.store(tracker.wanted(), Ordering::Release);
    let state = (tracker.drag.maybe(), tracker.drag.in_picture());
    if state != tracker.reported {
        tracker.reported = state;
        CHANGED.store(true, Ordering::Release);
    }
    result
}

fn log_end(line: Option<String>) {
    if let Some(line) = line {
        eprintln!("{}", line);
    }
}

/// A new session: nothing of the last one counts.
pub fn reset() {
    with_tracker(|t| {
        let reported = t.reported;
        let windows_host = t.drag.windows_host;
        *t = Tracker::default();
        t.reported = reported;
        t.drag.windows_host = windows_host;
    });
}

/// The cursor image a `host_cursor` message delivered.
pub fn note_cursor(cursor: CursorImage) {
    with_tracker(|t| t.note_cursor(cursor));
}

/// `pointer_maybe_in_picture` of a `host_cursor` message (absent = false).
pub fn note_maybe(maybe: bool) {
    log_end(with_tracker(|t| t.drag_mut().set_maybe(maybe)));
}

/// The host's system, from its capabilities.
pub fn note_window_drag_watched(watched: bool) {
    log_end(with_tracker(|t| t.drag_mut().set_window_drag_watched(watched)));
}

pub fn note_host_os(host_os: &str) {
    log_end(with_tracker(|t| t.drag_mut().set_host_os(host_os)));
}

/// The viewer's left button went down or up.
pub fn note_left_button(held: bool) {
    log_end(with_tracker(|t| t.drag_mut().set_left_held(held)));
}

/// An absolute pointer position was sent to the host.
pub fn note_position(x: i32, y: i32, width: i32, height: i32) {
    with_tracker(|t| t.note_position(x, y, width, height, Instant::now()));
}

/// (maybe, in the picture) when it changed since the last call: for the UI.
pub fn take_update() -> Option<(bool, bool)> {
    if !CHANGED.swap(false, Ordering::AcqRel) {
        return None;
    }
    Some(with_tracker(|t| t.reported))
}

/// Read by the decode thread for every frame: one atomic load. True only
/// during a drag that is not decided yet.
pub fn frame_wanted() -> bool {
    WANTED.load(Ordering::Acquire)
}

/// The decode thread has a frame: what to read of it, if a check is due
/// (at most one per `CHECK_INTERVAL`).
pub fn plan(picture_width: i32, picture_height: i32, bt709: bool) -> Option<Plan> {
    with_tracker(|t| t.plan(picture_width, picture_height, bt709, Instant::now()))
}

/// The picture could not be read: no check, the viewer's pointer stays.
pub fn abandon(plan: Plan) {
    with_tracker(|t| t.abandon(&plan));
}

/// Reads the luma a plan asks for (`plan.width` x `plan.height` values,
/// full range, row after row); `None` when the picture cannot be read.
pub type LumaReader = Box<dyn FnOnce(&Plan) -> Option<Vec<u8>> + Send>;

/// Hands a planned check to the thread that runs the checks: `read` gives
/// the luma there (already copied by the decode thread, or read from a
/// frame it kept a reference to), then the cursor is looked for. The
/// decode thread does not wait for any of it.
pub fn submit(plan: Plan, read: LumaReader) {
    static WORKER: std::sync::OnceLock<Option<std::sync::mpsc::Sender<(Plan, LumaReader)>>> =
        std::sync::OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<(Plan, LumaReader)>();
        std::thread::Builder::new()
            .name("pointer-in-picture".to_string())
            .spawn(move || {
                while let Ok((plan, read)) = rx.recv() {
                    match read(&plan) {
                        Some(luma) => {
                            let found = plan.check(&luma);
                            with_tracker(|t| t.finish(&plan, found));
                        }
                        None => with_tracker(|t| t.abandon(&plan)),
                    }
                }
            })
            .ok()
            .map(|_| tx)
    });
    match worker {
        Some(tx) => {
            if let Err(std::sync::mpsc::SendError((plan, _))) = tx.send((plan, read)) {
                abandon(plan);
            }
        }
        None => abandon(plan),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test arrow of the rule: 12 x 19, hot spot 0,0.
    const ARROW: [&str; 18] = [
        "X",
        "XX",
        "X.X",
        "X..X",
        "X...X",
        "X....X",
        "X.....X",
        "X......X",
        "X.......X",
        "X........X",
        "X.........X",
        "X......XXXXX",
        "X...X..X",
        "X..X X..X",
        "X.X  X..X",
        "XX    X..X",
        "X     X..X",
        "       XX",
    ];

    fn arrow() -> CursorImage {
        let (width, height) = (12usize, 19usize);
        let mut rgba = vec![0u8; width * height * 4];
        for (y, row) in ARROW.iter().enumerate() {
            for (x, c) in row.bytes().enumerate() {
                let px = match c {
                    b'X' => [0, 0, 0, 255],
                    b'.' => [255, 255, 255, 255],
                    _ => [0, 0, 0, 0],
                };
                rgba[(y * width + x) * 4..(y * width + x) * 4 + 4].copy_from_slice(&px);
            }
        }
        CursorImage { width: width as i32, height: height as i32, hot_x: 0, hot_y: 0, rgba }
    }

    struct Picture {
        width: i32,
        height: i32,
        luma: Vec<u8>,
    }

    struct Random(u64);
    impl Random {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 16) as u32
        }
    }

    impl Picture {
        fn flat(value: u8) -> Self {
            Picture { width: 320, height: 240, luma: vec![value; 320 * 240] }
        }

        fn noisy(seed: u64) -> Self {
            let mut random = Random(seed);
            Picture { width: 320, height: 240, luma: (0..320 * 240).map(|_| random.next() as u8).collect() }
        }

        /// Draws the cursor with its alpha, its hot spot at (x, y), `scale`
        /// times as large (nearest neighbour).
        fn draw(&mut self, cursor: &CursorImage, x: i32, y: i32, scale: f32) {
            let width = round_half_up(cursor.width as f64 * scale as f64);
            let height = round_half_up(cursor.height as f64 * scale as f64);
            let left = x - round_half_up(cursor.hot_x as f64 * scale as f64);
            let top = y - round_half_up(cursor.hot_y as f64 * scale as f64);
            for dy in 0..height {
                let sy = ((dy as f64 + 0.5) * cursor.height as f64 / height as f64).floor() as usize;
                for dx in 0..width {
                    let sx = ((dx as f64 + 0.5) * cursor.width as f64 / width as f64).floor() as usize;
                    let px = &cursor.rgba[(sy * cursor.width as usize + sx) * 4..][..4];
                    let (tx, ty) = (left + dx, top + dy);
                    if tx < 0 || ty < 0 || tx >= self.width || ty >= self.height {
                        continue;
                    }
                    let at = (ty * self.width + tx) as usize;
                    let alpha = px[3] as f64 / 255.0;
                    let luma = pixel_luma(px[0], px[1], px[2], false) as f64;
                    self.luma[at] = (luma * alpha + self.luma[at] as f64 * (1.0 - alpha)).round() as u8;
                }
            }
        }

        fn box_blur(&mut self) {
            let src = self.luma.clone();
            for y in 0..self.height {
                for x in 0..self.width {
                    let mut sum = 0u32;
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let sx = (x + dx).clamp(0, self.width - 1);
                            let sy = (y + dy).clamp(0, self.height - 1);
                            sum += src[(sy * self.width + sx) as usize] as u32;
                        }
                    }
                    self.luma[(y * self.width + x) as usize] = ((sum + 4) / 9) as u8;
                }
            }
        }

        /// Adds noise of -amount..=amount to every pixel.
        fn add_noise(&mut self, amount: i32, seed: u64) {
            let mut random = Random(seed);
            for value in &mut self.luma {
                let noise = (random.next() % (2 * amount as u32 + 1)) as i32 - amount;
                *value = (*value as i32 + noise).clamp(0, 255) as u8;
            }
        }

        fn view(&self) -> LumaView<'_> {
            LumaView { data: &self.luma, stride: self.width as usize, width: self.width, height: self.height, x0: 0, y0: 0 }
        }
    }

    fn found(picture: &Picture, cursor: &CursorImage, positions: &[(i32, i32)]) -> Option<f32> {
        check(&templates(cursor, false), &picture.view(), positions)
    }

    #[test]
    fn the_arrow_is_a_usable_template() {
        let all = templates(&arrow(), false);
        assert_eq!(all.len(), 5);
        assert_eq!(all.iter().map(|t| t.scale).collect::<Vec<_>>(), SCALES);
        assert_eq!((all[0].width, all[0].height), (12, 19));
        assert_eq!((all[2].width, all[2].height), (18, 29)); // round(28.5) = 29
        assert_eq!((all[4].width, all[4].height), (24, 38));
        assert!(all[0].ink_pixels() >= MIN_INK_PIXELS);
    }

    // 1. the arrow on a noisy background
    #[test]
    fn found_on_a_noisy_background() {
        let mut picture = Picture::noisy(0x1234_5678_9abc_def1);
        picture.draw(&arrow(), 150, 100, 1.0);
        assert_eq!(found(&picture, &arrow(), &[(150, 100)]), Some(1.0));
        // Up to three pixels off the position sent, either way.
        assert_eq!(found(&picture, &arrow(), &[(153, 97)]), Some(1.0));
        assert_eq!(found(&picture, &arrow(), &[(154, 100)]), None);
        // Any of the positions.
        assert_eq!(found(&picture, &arrow(), &[(20, 30), (148, 102), (200, 200)]), Some(1.0));
    }

    // 2. on a bright and on a dark background
    #[test]
    fn found_on_bright_and_dark_backgrounds() {
        for background in [235u8, 20] {
            let mut picture = Picture::flat(background);
            picture.draw(&arrow(), 60, 200, 1.0);
            assert_eq!(found(&picture, &arrow(), &[(61, 199)]), Some(1.0), "background {}", background);
        }
    }

    // 3. the picture blurred and noisy
    #[test]
    fn found_with_noise_and_scores_when_blurred() {
        let cursor = arrow();
        let all = templates(&cursor, false);
        // +-6 of noise alone: found, on every background.
        for (name, mut picture) in [("noisy", Picture::noisy(7)), ("bright", Picture::flat(235)), ("dark", Picture::flat(20))] {
            picture.draw(&cursor, 150, 100, 1.0);
            picture.add_noise(6, 99);
            assert_eq!(found(&picture, &cursor, &[(150, 100)]), Some(1.0), "{}", name);
        }
        // A 3x3 box blur of the composed picture, then the noise. The
        // arrow's outline is one pixel wide: the blur leaves a third of its
        // contrast, and the mean difference (66 to 94) is that of a picture
        // without an arrow (not_found_without_an_arrow: 80 and more). With
        // the rule's numbers such a picture is "not found": the viewer's
        // pointer stays, two pointers. A quarter of that blur mixed
        // into the sharp picture is still found.
        for (name, picture) in [("noisy", Picture::noisy(7)), ("bright", Picture::flat(235)), ("dark", Picture::flat(20)), ("grey", Picture::flat(128))] {
            let mut sharp = picture;
            sharp.draw(&cursor, 150, 100, 1.0);
            let mut blurred = Picture { width: sharp.width, height: sharp.height, luma: sharp.luma.clone() };
            blurred.box_blur();
            let mixed = |part: f64| {
                let mut out = Picture { width: sharp.width, height: sharp.height, luma: sharp.luma.clone() };
                for (i, value) in out.luma.iter_mut().enumerate() {
                    *value = (sharp.luma[i] as f64 * (1.0 - part) + blurred.luma[i] as f64 * part).round() as u8;
                }
                out.add_noise(6, 99);
                out
            };
            let best = |picture: &Picture| {
                (-3..=3)
                    .flat_map(|dy| (-3..=3).map(move |dx| (dx, dy)))
                    .filter_map(|(dx, dy)| score(&all[0], &picture.view(), 150 + dx, 100 + dy))
                    .fold(f32::MAX, f32::min)
            };
            let full = mixed(1.0);
            let quarter = mixed(0.25);
            eprintln!("blurred 3x3 + noise on {}: best score {:.1}; a quarter of the blur: {:.1}", name, best(&full), best(&quarter));
            assert_eq!(found(&full, &cursor, &[(150, 100)]), None, "{}", name);
            assert_eq!(found(&quarter, &cursor, &[(150, 100)]), Some(1.0), "{}", name);
        }
    }

    // 4. no arrow in the picture
    #[test]
    fn not_found_without_an_arrow() {
        for picture in [Picture::noisy(0x1234_5678_9abc_def1), Picture::flat(235), Picture::flat(20), Picture::flat(128)] {
            let positions: Vec<(i32, i32)> = (0..16).map(|i| (20 + i * 17, 30 + i * 11)).collect();
            assert_eq!(found(&picture, &arrow(), &positions), None);
            // Nowhere near a match: the best placement of all is far above 26.
            let all = templates(&arrow(), false);
            let best = positions
                .iter()
                .flat_map(|&(x, y)| (-3..=3).flat_map(move |dy| (-3..=3).map(move |dx| (x + dx, y + dy))))
                .filter_map(|(x, y)| score(&all[0], &picture.view(), x, y))
                .fold(f32::MAX, f32::min);
            assert!(best > 60.0, "best score without an arrow: {}", best);
        }
    }

    // 5. an arrow in the picture, but 60 px away from every position
    #[test]
    fn not_found_away_from_the_positions() {
        for mut picture in [Picture::noisy(3), Picture::flat(235), Picture::flat(20)] {
            picture.draw(&arrow(), 150, 100, 1.0);
            assert_eq!(found(&picture, &arrow(), &[(210, 100), (150, 160), (90, 100), (150, 40)]), None);
        }
    }

    // 6. the arrow drawn 1.5 times as large
    #[test]
    fn found_at_one_and_a_half_times_the_size() {
        for mut picture in [Picture::noisy(5), Picture::flat(235), Picture::flat(20)] {
            picture.draw(&arrow(), 150, 100, 1.5);
            assert_eq!(found(&picture, &arrow(), &[(151, 101)]), Some(1.5));
        }
    }

    // 7. cursors that cannot be told from a background
    #[test]
    fn small_and_one_coloured_cursors_are_never_found() {
        // 6 x 6 opaque black and white: 36 ink pixels.
        let mut small = CursorImage { width: 6, height: 6, hot_x: 0, hot_y: 0, rgba: vec![255; 6 * 6 * 4] };
        for i in 0..18 {
            small.rgba[i * 4..i * 4 + 3].copy_from_slice(&[0, 0, 0]);
        }
        assert!(templates(&small, false).is_empty());
        let mut picture = Picture::flat(128);
        picture.draw(&small, 100, 100, 1.0);
        assert_eq!(found(&picture, &small, &[(100, 100)]), None);

        let white = CursorImage { width: 16, height: 16, hot_x: 0, hot_y: 0, rgba: vec![255; 16 * 16 * 4] };
        assert!(templates(&white, false).is_empty());
        let mut picture = Picture::flat(20);
        picture.draw(&white, 100, 100, 1.0);
        assert_eq!(found(&picture, &white, &[(100, 100)]), None);
        // Nothing to find it in on a white picture either.
        assert_eq!(found(&Picture::flat(255), &white, &[(100, 100)]), None);
    }

    #[test]
    fn every_drag_on_a_windows_host_looks() {
        // Apps that move their own windows: Windows draws the pointer into
        // the picture and the host cannot say so.
        let mut drag = Drag::default();
        drag.set_host_os("windows");
        drag.set_left_held(true);
        assert!(drag.wants_check());
        drag.set_left_held(false);
        drag.set_host_os("linux");
        drag.set_left_held(true);
        assert!(!drag.wants_check(), "elsewhere only when the host says maybe");
        drag.set_maybe(true);
        assert!(drag.wants_check());
        // A Windows agent that watches window drags itself: not looked at.
        drag.set_left_held(false);
        drag.set_maybe(false);
        drag.set_host_os("windows");
        drag.set_window_drag_watched(true);
        drag.set_left_held(true);
        assert!(!drag.wants_check());
    }

    // 8. the decision over a drag, and its log line
    #[test]
    fn three_consecutive_checks_decide() {
        let mut drag = Drag::default();
        assert_eq!(drag.set_left_held(true), None);
        assert!(!drag.wants_check());
        assert_eq!(drag.set_maybe(true), None);
        assert!(drag.wants_check());
        for (i, found) in [Some(1.0), Some(1.0), None, Some(1.0), Some(1.0)].into_iter().enumerate() {
            drag.record_check(found);
            assert!(!drag.in_picture(), "after check {}", i + 1);
        }
        drag.record_check(Some(1.0));
        assert!(drag.in_picture());
        assert!(!drag.wants_check(), "no more checks once found");
        drag.record_check(None);
        assert!(drag.in_picture(), "stays for the rest of the drag");
        assert_eq!(
            drag.set_left_held(false).as_deref(),
            Some("pointer looked for in the picture: found after 6 checks")
        );
        assert!(!drag.in_picture());

        // A drag in which it is never found; "maybe" going false ends it.
        assert_eq!(drag.set_left_held(true), None);
        assert!(drag.wants_check());
        for _ in 0..4 {
            drag.record_check(None);
            assert!(!drag.in_picture());
        }
        assert_eq!(
            drag.set_maybe(false).as_deref(),
            Some("pointer looked for in the picture: not found (4 checks)")
        );
        assert_eq!(drag.set_left_held(false), None, "one line per drag");
        // "maybe" without the button held is no drag.
        assert_eq!(drag.set_maybe(true), None);
        assert!(!drag.wants_check());
        assert_eq!(drag.set_maybe(false), None);
    }

    #[test]
    fn a_drag_that_never_finds_it_checks_less_often() {
        let mut drag = Drag::default();
        drag.set_host_os("windows");
        drag.set_left_held(true);
        for _ in 0..FAST_MISSES {
            assert_eq!(drag.check_interval(), CHECK_INTERVAL);
            drag.record_check(None);
        }
        assert_eq!(drag.check_interval(), SLOW_CHECK_INTERVAL);
        drag.record_check(Some(1.0));
        assert_eq!(drag.check_interval(), CHECK_INTERVAL, "fast again once found");
        for _ in 0..FAST_MISSES {
            drag.record_check(None);
        }
        drag.set_left_held(false);
        drag.set_left_held(true);
        assert_eq!(drag.check_interval(), CHECK_INTERVAL, "a new drag starts fast");
    }

    #[test]
    fn the_scale_that_matched_is_kept_for_the_drag() {
        let mut drag = Drag::default();
        drag.set_left_held(true);
        drag.set_maybe(true);
        assert_eq!(drag.scale(), None);
        drag.record_check(Some(1.5));
        drag.record_check(None);
        assert_eq!(drag.scale(), Some(1.5));
        drag.set_left_held(false);
        drag.set_left_held(true);
        assert_eq!(drag.scale(), None, "a new drag tries every scale again");
    }

    #[test]
    fn positions_are_scaled_newest_first_and_without_repeats() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut tracker = Tracker::default();
        // Shown 960 x 540, the picture is 1920 x 1080.
        tracker.note_position(100, 50, 960, 540, at(500));
        tracker.note_position(101, 50, 960, 540, at(510)); // 2 px on: a repeat of the next
        tracker.note_position(102, 50, 960, 540, at(520));
        tracker.note_position(300, 200, 960, 540, at(700));
        tracker.note_position(300, 200, 960, 540, at(900)); // sent again: one position
        assert_eq!(tracker.positions(1920, 1080, at(1000)), vec![(600, 400), (204, 100), (200, 100)]);
        // 600 ms after they were sent the older ones are gone; the newest stays.
        assert_eq!(tracker.positions(1920, 1080, at(1400)), vec![(600, 400)]);
        assert_eq!(tracker.positions(1920, 1080, at(60_000)), vec![(600, 400)]);
        // Never more than 16.
        for i in 0..40 {
            tracker.note_position(10 + i * 8, 10, 1920, 1080, at(2000 + i as u64));
        }
        let positions = tracker.positions(1920, 1080, at(2040));
        assert_eq!(positions.len(), MAX_POSITIONS);
        assert_eq!(positions[0], (10 + 39 * 8, 10));
    }

    #[test]
    fn a_drag_through_the_tracker() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let cursor = arrow();
        let mut picture = Picture::noisy(11);
        picture.draw(&cursor, 150, 100, 1.5);

        let mut tracker = Tracker::default();
        tracker.note_cursor(cursor.clone());
        tracker.note_position(150, 100, 320, 240, at(0));
        assert!(!tracker.wanted(), "no drag yet");
        assert!(tracker.plan(320, 240, false, at(0)).is_none());
        tracker.drag_mut().set_left_held(true);
        tracker.drag_mut().set_maybe(true);
        assert!(tracker.wanted());

        let read = |plan: &Plan| -> Vec<u8> {
            let mut out = Vec::new();
            for y in plan.y0..plan.y0 + plan.height {
                let row = (y * picture.width + plan.x0) as usize;
                out.extend_from_slice(&picture.luma[row..row + plan.width as usize]);
            }
            out
        };

        let plan = tracker.plan(320, 240, false, at(100)).expect("a check is due");
        // The part to read: the position padded by the largest template + 3.
        assert_eq!((plan.x0, plan.y0, plan.width, plan.height), (150 - 41, 100 - 41, 83, 83));
        assert!(!tracker.wanted(), "one check at a time");
        let found = plan.check(&read(&plan));
        assert_eq!(found, Some(1.5));
        tracker.finish(&plan, found);

        assert!(tracker.plan(320, 240, false, at(120)).is_none(), "at most one check per 30 ms");
        let plan = tracker.plan(320, 240, false, at(130)).expect("due again");
        // Only the scale that matched: a smaller part.
        assert_eq!((plan.x0, plan.y0, plan.width, plan.height), (150 - 32, 100 - 32, 65, 65));
        let found = plan.check(&read(&plan));
        tracker.finish(&plan, found);
        assert!(!tracker.drag().in_picture());
        let plan = tracker.plan(320, 240, false, at(160)).unwrap();
        let found = plan.check(&read(&plan));
        tracker.finish(&plan, found);
        assert!(tracker.drag().in_picture());
        assert!(!tracker.wanted(), "decided: no more frames");
        assert!(tracker.plan(320, 240, false, at(400)).is_none());

        // A check that ends after its drag says nothing about the next one.
        tracker.drag_mut().set_left_held(false);
        tracker.drag_mut().set_left_held(true);
        let stale = tracker.plan(320, 240, false, at(500)).unwrap();
        tracker.drag_mut().set_left_held(false);
        tracker.drag_mut().set_left_held(true);
        tracker.finish(&stale, Some(1.5));
        assert!(tracker.drag().wants_check());
        let plan = tracker.plan(320, 240, false, at(600)).unwrap();
        // A picture that could not be read: no check, and the next frame is wanted.
        tracker.abandon(&plan);
        assert!(tracker.wanted());
        assert_eq!(
            tracker.drag_mut().set_left_held(false).as_deref(),
            Some("pointer looked for in the picture: not found (0 checks)")
        );
    }

    #[test]
    fn a_cursor_that_cannot_be_found_is_a_check_without_a_picture() {
        let start = Instant::now();
        let mut tracker = Tracker::default();
        tracker.note_cursor(CursorImage { width: 16, height: 16, hot_x: 0, hot_y: 0, rgba: vec![255; 16 * 16 * 4] });
        tracker.note_position(10, 10, 320, 240, start);
        tracker.drag_mut().set_left_held(true);
        tracker.drag_mut().set_maybe(true);
        assert!(tracker.plan(320, 240, false, start).is_none());
        assert_eq!(
            tracker.drag_mut().set_maybe(false).as_deref(),
            Some("pointer looked for in the picture: not found (1 checks)")
        );
    }
}
