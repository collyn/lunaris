//! The USB mouse itself: its descriptors and the reports it sends.
//!
//! A plain HID mouse (no boot protocol): 5 buttons, 16-bit relative X/Y (a
//! fast move fits one report), a wheel and a horizontal wheel (AC Pan).
//! Windows drives it with its own mouse driver (mouhid), so the moves are a
//! mouse's, not injected input.

/// pid.codes (open-source USB IDs). 0x0001 is its test product ID.
pub const VENDOR_ID: u16 = 0x1209;
pub const PRODUCT_ID: u16 = 0x0001;
/// Interrupt IN endpoint 1.
pub const ENDPOINT_IN: u8 = 0x81;
/// Polled every millisecond (full speed), as gaming mice are.
pub const POLL_INTERVAL_MS: u8 = 1;
/// Bytes in a report (see REPORT_DESCRIPTOR).
pub const REPORT_LEN: usize = 7;

pub const REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x02, // Usage (Mouse)
    0xA1, 0x01, // Collection (Application)
    0x09, 0x01, //   Usage (Pointer)
    0xA1, 0x00, //   Collection (Physical)
    0x05, 0x09, //     Usage Page (Buttons)
    0x19, 0x01, //     Usage Minimum (1)
    0x29, 0x05, //     Usage Maximum (5)
    0x15, 0x00, //     Logical Minimum (0)
    0x25, 0x01, //     Logical Maximum (1)
    0x95, 0x05, //     Report Count (5)
    0x75, 0x01, //     Report Size (1)
    0x81, 0x02, //     Input (Data, Variable, Absolute)
    0x95, 0x01, //     Report Count (1)
    0x75, 0x03, //     Report Size (3)
    0x81, 0x03, //     Input (Constant): padding
    0x05, 0x01, //     Usage Page (Generic Desktop)
    0x09, 0x30, //     Usage (X)
    0x09, 0x31, //     Usage (Y)
    0x16, 0x01, 0x80, // Logical Minimum (-32767)
    0x26, 0xFF, 0x7F, // Logical Maximum (32767)
    0x75, 0x10, //     Report Size (16)
    0x95, 0x02, //     Report Count (2)
    0x81, 0x06, //     Input (Data, Variable, Relative)
    0x09, 0x38, //     Usage (Wheel)
    0x15, 0x81, //     Logical Minimum (-127)
    0x25, 0x7F, //     Logical Maximum (127)
    0x75, 0x08, //     Report Size (8)
    0x95, 0x01, //     Report Count (1)
    0x81, 0x06, //     Input (Data, Variable, Relative)
    0x05, 0x0C, //     Usage Page (Consumer)
    0x0A, 0x38, 0x02, // Usage (AC Pan)
    0x15, 0x81, //     Logical Minimum (-127)
    0x25, 0x7F, //     Logical Maximum (127)
    0x75, 0x08, //     Report Size (8)
    0x95, 0x01, //     Report Count (1)
    0x81, 0x06, //     Input (Data, Variable, Relative)
    0xC0, //   End Collection
    0xC0, // End Collection
];

const MANUFACTURER: &str = "Lunaris";
const PRODUCT: &str = "Lunaris Game Mouse";
const SERIAL: &str = "LUNARIS-GM1";

pub fn device_descriptor() -> Vec<u8> {
    let [vl, vh] = VENDOR_ID.to_le_bytes();
    let [pl, ph] = PRODUCT_ID.to_le_bytes();
    vec![
        18, 0x01, // bLength, DEVICE
        0x00, 0x02, // bcdUSB 2.00
        0x00, 0x00, 0x00, // class/subclass/protocol: per interface
        64,   // bMaxPacketSize0
        vl, vh, pl, ph, // idVendor, idProduct
        0x00, 0x01, // bcdDevice 1.00
        1, 2, 3, // iManufacturer, iProduct, iSerialNumber
        1,    // bNumConfigurations
    ]
}

/// The HID class descriptor (between the interface and its endpoint).
pub fn hid_descriptor() -> Vec<u8> {
    let [ll, lh] = (REPORT_DESCRIPTOR.len() as u16).to_le_bytes();
    vec![9, 0x21, 0x11, 0x01, 0x00, 0x01, 0x22, ll, lh]
}

pub fn configuration_descriptor() -> Vec<u8> {
    let mut rest = Vec::new();
    // Interface 0: HID, no boot subclass, one endpoint.
    rest.extend_from_slice(&[9, 0x04, 0, 0, 1, 0x03, 0x00, 0x00, 0]);
    rest.extend_from_slice(&hid_descriptor());
    let [ml, mh] = (REPORT_LEN as u16 + 1).to_le_bytes();
    rest.extend_from_slice(&[7, 0x05, ENDPOINT_IN, 0x03, ml, mh, POLL_INTERVAL_MS]);
    let total = (9 + rest.len()) as u16;
    let [tl, th] = total.to_le_bytes();
    // Bus powered with remote wake-up, 100 mA.
    let mut out = vec![9, 0x02, tl, th, 1, 1, 0, 0xA0, 50];
    out.extend_from_slice(&rest);
    out
}

/// String descriptor `index` (0: the languages, US English only).
pub fn string_descriptor(index: u8) -> Option<Vec<u8>> {
    let text = match index {
        0 => return Some(vec![4, 0x03, 0x09, 0x04]),
        1 => MANUFACTURER,
        2 => PRODUCT,
        3 => SERIAL,
        _ => return None,
    };
    let mut out = vec![0, 0x03];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out[0] = out.len() as u8;
    Some(out)
}

/// One report: buttons (bit 0 left, 1 right, 2 middle, 3 back, 4 forward),
/// a relative move, wheel and horizontal wheel clicks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub buttons: u8,
    pub dx: i16,
    pub dy: i16,
    pub wheel: i8,
    pub pan: i8,
}

impl Report {
    pub fn bytes(&self) -> [u8; REPORT_LEN] {
        let [xl, xh] = self.dx.to_le_bytes();
        let [yl, yh] = self.dy.to_le_bytes();
        [self.buttons & 0x1F, xl, xh, yl, yh, self.wheel as u8, self.pan as u8]
    }
}

/// Largest move of one report (the descriptor's logical range).
const MAX_MOVE: i32 = 32767;

/// Reports waiting for the host to read them. Moves with nothing else in
/// between are summed into one report (the host reads every millisecond, so
/// the sum is what a real mouse would have reported); a button or wheel
/// change starts a new one, so presses and releases are never merged away.
#[derive(Debug, Default)]
pub struct ReportQueue {
    buttons: u8,
    /// With when the first input in it came (how long it waited, measured).
    pending: std::collections::VecDeque<(Report, std::time::Instant)>,
    /// The last report waiting is a move only (one a move may be summed into:
    /// never a press or release, whose position must stay where it was).
    last_is_move: bool,
}

/// Reports kept when nobody reads them (the device is not attached): the
/// oldest go first.
const MAX_PENDING: usize = 256;

impl ReportQueue {
    pub fn buttons(&self) -> u8 {
        self.buttons
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn pop(&mut self) -> Option<Report> {
        self.pending.pop_front().map(|(r, _)| r)
    }

    /// The next report and how long since its first input came.
    pub fn pop_timed(&mut self) -> Option<(Report, std::time::Duration)> {
        self.pending.pop_front().map(|(r, since)| (r, since.elapsed()))
    }

    fn push(&mut self, report: Report, is_move: bool) {
        if self.pending.len() >= MAX_PENDING {
            self.pending.pop_front();
        }
        self.pending.push_back((report, std::time::Instant::now()));
        self.last_is_move = is_move;
    }

    pub fn move_by(&mut self, dx: i32, dy: i32) {
        let (mut dx, mut dy) = (dx, dy);
        // Into the last report when it is a move of the same buttons.
        if let Some((last, _)) = self.pending.back_mut().filter(|_| self.last_is_move) {
            if last.buttons == self.buttons {
                let x = (last.dx as i32 + dx).clamp(-MAX_MOVE, MAX_MOVE);
                let y = (last.dy as i32 + dy).clamp(-MAX_MOVE, MAX_MOVE);
                dx -= x - last.dx as i32;
                dy -= y - last.dy as i32;
                last.dx = x as i16;
                last.dy = y as i16;
            }
        }
        while dx != 0 || dy != 0 {
            let x = dx.clamp(-MAX_MOVE, MAX_MOVE);
            let y = dy.clamp(-MAX_MOVE, MAX_MOVE);
            self.push(Report { buttons: self.buttons, dx: x as i16, dy: y as i16, ..Default::default() }, true);
            dx -= x;
            dy -= y;
        }
    }

    /// Presses (`down`) or releases button bit `bit` (0..=4).
    pub fn button(&mut self, bit: u8, down: bool) {
        if bit > 4 {
            return;
        }
        let buttons = if down { self.buttons | (1 << bit) } else { self.buttons & !(1 << bit) };
        if buttons == self.buttons {
            return;
        }
        self.buttons = buttons;
        self.push(Report { buttons, ..Default::default() }, false);
    }

    /// Wheel clicks (`vertical`: up is positive; else right is positive).
    pub fn wheel(&mut self, clicks: i32, vertical: bool) {
        let mut left = clicks;
        while left != 0 {
            let step = left.clamp(-127, 127);
            let (wheel, pan) = if vertical { (step as i8, 0) } else { (0, step as i8) };
            self.push(Report { buttons: self.buttons, wheel, pan, ..Default::default() }, false);
            left -= step;
        }
    }

    /// Lets go of every button (the viewer left).
    pub fn release_all(&mut self) {
        if self.buttons != 0 {
            self.buttons = 0;
            self.push(Report::default(), false);
        }
    }

    /// Forgets what was not read (the host detached the device).
    pub fn clear(&mut self) {
        self.pending.clear();
        self.buttons = 0;
        self.last_is_move = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_have_the_lengths_they_say() {
        let device = device_descriptor();
        assert_eq!(device.len(), device[0] as usize);
        let config = configuration_descriptor();
        assert_eq!(config.len(), u16::from_le_bytes([config[2], config[3]]) as usize);
        assert_eq!(config.len(), 9 + 9 + 9 + 7);
        let hid = hid_descriptor();
        assert_eq!(u16::from_le_bytes([hid[7], hid[8]]) as usize, REPORT_DESCRIPTOR.len());
        let product = string_descriptor(2).unwrap();
        assert_eq!(product.len(), product[0] as usize);
        assert_eq!(String::from_utf16(&product[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()).unwrap(), "Lunaris Game Mouse");
        assert!(string_descriptor(9).is_none());
    }

    #[test]
    fn the_report_descriptor_describes_seven_bytes() {
        // 5 buttons + 3 padding bits, X and Y of 16 bits, wheel, pan.
        assert_eq!((5 + 3 + 16 * 2 + 8 + 8) / 8, REPORT_LEN);
        let r = Report { buttons: 0b10011, dx: -2, dy: 300, wheel: -1, pan: 2 };
        assert_eq!(r.bytes(), [0b10011, 0xFE, 0xFF, 0x2C, 0x01, 0xFF, 0x02]);
    }

    #[test]
    fn moves_are_summed_until_something_else_happens() {
        let mut q = ReportQueue::default();
        q.move_by(3, -4);
        q.move_by(5, 1);
        assert_eq!(q.len(), 1);
        q.button(0, true);
        q.move_by(1, 1);
        q.button(0, false);
        q.move_by(2, 0);
        let all: Vec<Report> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(
            all,
            vec![
                Report { dx: 8, dy: -3, ..Default::default() },
                Report { buttons: 1, ..Default::default() },
                Report { buttons: 1, dx: 1, dy: 1, ..Default::default() },
                Report { buttons: 0, ..Default::default() },
                Report { dx: 2, ..Default::default() },
            ]
        );
    }

    #[test]
    fn a_press_and_its_release_stay_two_reports() {
        let mut q = ReportQueue::default();
        q.button(1, true);
        q.button(1, false);
        assert_eq!(q.pop().unwrap().buttons, 0b10);
        assert_eq!(q.pop().unwrap().buttons, 0);
        assert!(q.pop().is_none());
        // Pressing what is already down sends nothing.
        q.button(1, false);
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn a_move_beyond_one_report_is_split() {
        let mut q = ReportQueue::default();
        q.move_by(40_000, -70_000);
        let all: Vec<Report> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(all.iter().map(|r| r.dx as i32).sum::<i32>(), 40_000);
        assert_eq!(all.iter().map(|r| r.dy as i32).sum::<i32>(), -70_000);
        assert!(all.iter().all(|r| r.dx.unsigned_abs() <= 32767 && r.dy.unsigned_abs() <= 32767));
    }

    #[test]
    fn wheel_clicks_and_unread_reports() {
        let mut q = ReportQueue::default();
        q.wheel(-200, true);
        q.wheel(3, false);
        let all: Vec<Report> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(all.iter().map(|r| r.wheel as i32).sum::<i32>(), -200);
        assert_eq!(all.last().unwrap().pan, 3);
        for _ in 0..(MAX_PENDING + 10) {
            q.button(0, true);
            q.button(0, false);
        }
        assert_eq!(q.len(), MAX_PENDING);
        q.button(2, true);
        q.release_all();
        assert_eq!(q.buttons(), 0);
        q.clear();
        assert_eq!(q.len(), 0);
    }
}
