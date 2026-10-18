//! Календарь без UI и без сети: модель события (JSCalendar, RFC 8984), раскладка
//! пересекающихся событий в дне, занятость участников и поиск общего свободного
//! времени («Помощник планирования»). Одинаково служит egui- и gpui-интерфейсу.

mod event;
mod freebusy;
mod invite;
mod layout;

pub use event::{Event, EventStatus, FreeBusy, NewEvent, Participant, Partstat, format_duration, parse_duration};
pub use freebusy::{Busy, BusyKind, Cell, WorkHours, find_slots, grid, merge};
pub use invite::{Invitation, imip_reply, participant_id, partstat_json, reply_subject};
pub use layout::{Placed, layout_day};

/// Промежуток времени в Unix-секундах, `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub start: i64,
    pub end: i64,
}

impl Span {
    pub fn new(start: i64, end: i64) -> Self {
        Self { start, end: end.max(start) }
    }

    pub fn overlaps(&self, o: &Span) -> bool {
        self.start < o.end && o.start < self.end
    }

    pub fn len(&self) -> i64 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
