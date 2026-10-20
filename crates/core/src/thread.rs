//! Беседа (S5-05): письма одной цепочки из всех папок (входящие, «Отправленные», архив) по времени, свои и чужие,
//! разделители по дням. Тела и разбор — отдельно: здесь только порядок и разметка ленты.

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use postman_cache::EmailRow;

/// Письмо беседы.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub row: EmailRow,
    /// Отправлено мной: адрес «От» — один из моих (Identity, в том числе общих ящиков).
    pub mine: bool,
    /// День получения в часовом поясе ленты.
    pub day: Date,
}

/// Строка ленты: разделитель дня или письмо (индекс в [`Conversation::messages`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Day(Date),
    Message(usize),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Conversation {
    /// Ключ цепочки в кэше (`EmailRow::thread_id`).
    pub thread: String,
    /// Старые первыми.
    pub messages: Vec<Message>,
    pub items: Vec<Item>,
}

impl Conversation {
    /// Собрать ленту: письма по времени получения (при равном — по id), повторы id отброшены; перед первым письмом
    /// каждого дня — разделитель. `me` — мои адреса, без учёта регистра.
    pub fn assemble(thread: &str, mut rows: Vec<EmailRow>, me: &[String], tz: &TimeZone) -> Self {
        rows.sort_by(|a, b| a.received_at.cmp(&b.received_at).then_with(|| a.id.cmp(&b.id)));
        rows.dedup_by(|a, b| a.id == b.id);
        let mut c = Self { thread: thread.to_owned(), ..Self::default() };
        for row in rows {
            let day =
                Timestamp::from_second(row.received_at).map(|t| t.to_zoned(tz.clone()).date()).unwrap_or_default();
            if c.messages.last().is_none_or(|m| m.day != day) {
                c.items.push(Item::Day(day));
            }
            c.items.push(Item::Message(c.messages.len()));
            let mine = is_mine(&row.from_addr, me);
            c.messages.push(Message { row, mine, day });
        }
        c
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Место письма в беседе.
    pub fn position(&self, id: &str) -> Option<usize> {
        self.messages.iter().position(|m| m.row.id == id)
    }

    /// Соседнее письмо (Ctrl+→ / Ctrl+←): `delta` писем от `id`, у края — `None`.
    pub fn neighbor(&self, id: &str, delta: isize) -> Option<&EmailRow> {
        let at = self.position(id)?.checked_add_signed(delta)?;
        self.messages.get(at).map(|m| &m.row)
    }
}

/// Адрес — мой (без учёта регистра).
pub fn is_mine(addr: &str, me: &[String]) -> bool {
    !addr.is_empty() && me.iter().any(|m| m.eq_ignore_ascii_case(addr))
}

/// Пересылка по теме («FW:», «Fwd:», «Пересл:»): пересланное письмо — содержимое, в ленте показывается целиком.
pub fn is_forward(subject: &str) -> bool {
    let s = subject.trim_start().to_lowercase();
    ["fw:", "fwd:", "пересл:", "переслано:"].iter().any(|p| s.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, at: i64, from: &str) -> EmailRow {
        EmailRow {
            id: id.into(),
            thread_id: "t".into(),
            mailbox_ids: vec!["in".into()],
            received_at: at,
            subject: "Re: План".into(),
            from_name: from.into(),
            from_addr: from.into(),
            preview: String::new(),
            seen: true,
            flagged: false,
            has_attachment: false,
            size: 1,
            keywords: Vec::new(),
        }
    }

    #[test]
    fn ordered_by_time_with_days_and_mine() {
        let tz = TimeZone::UTC;
        // 2026-10-19 23:00, 2026-10-20 09:00 и 10:00 UTC; своё письмо — из «Отправленных».
        let (d1, d2) = (1_792_450_800, 1_792_486_800);
        let rows = vec![
            row("c", d2 + 3600, "ivan@example.com"),
            row("a", d1, "ivan@example.com"),
            row("b", d2, "ME@example.com"),
        ];
        let c = Conversation::assemble("t", rows.clone(), &["me@example.com".into()], &tz);
        let ids: Vec<&str> = c.messages.iter().map(|m| m.row.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(c.messages.iter().map(|m| m.mine).collect::<Vec<_>>(), [false, true, false]);
        let day = |s: &str| s.parse::<Date>().unwrap();
        assert_eq!(
            c.items,
            [
                Item::Day(day("2026-10-19")),
                Item::Message(0),
                Item::Day(day("2026-10-20")),
                Item::Message(1),
                Item::Message(2)
            ]
        );
        // В часовом поясе +03:00 все три — 20-го: один разделитель.
        let msk = TimeZone::fixed(jiff::tz::offset(3));
        assert_eq!(
            Conversation::assemble("t", rows, &[], &msk).items.iter().filter(|i| matches!(i, Item::Day(_))).count(),
            1
        );
    }

    #[test]
    fn neighbors_and_duplicates() {
        let rows = vec![row("a", 1, "x@e"), row("b", 2, "x@e"), row("a", 1, "x@e")];
        let c = Conversation::assemble("t", rows, &[], &TimeZone::UTC);
        assert_eq!(c.len(), 2);
        assert_eq!(c.neighbor("a", 1).map(|r| r.id.as_str()), Some("b"));
        assert_eq!(c.neighbor("b", -1).map(|r| r.id.as_str()), Some("a"));
        assert!(c.neighbor("b", 1).is_none() && c.neighbor("a", -1).is_none() && c.neighbor("zz", 1).is_none());
        // Пустой адрес «От» — не мой, даже если среди моих есть пустой.
        assert!(!is_mine("", &[String::new()]));
    }

    #[test]
    fn forwards_by_subject() {
        assert!(is_forward("FW: Смета") && is_forward("Fwd: budget") && is_forward(" Пересл: x"));
        assert!(!is_forward("Re: FW: x") && !is_forward("Fwdx"));
    }
}
