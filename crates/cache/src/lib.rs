//! Локальный кэш на SQLite. Хранит только то, что нужно для списка и быстрого
//! старта; тела писем — лениво, по запросу. UI читает отсюда окнами
//! (`page`), а не держит ящик в памяти.

use rusqlite::{Connection, OptionalExtension, params};

pub use rusqlite::Error;
pub type Result<T> = rusqlite::Result<T>;

#[derive(Debug, Clone, PartialEq)]
pub struct MailboxRow {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub role: Option<String>,
    pub sort_order: u32,
    pub total: u32,
    pub unread: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmailRow {
    pub id: String,
    pub thread_id: String,
    pub mailbox_ids: Vec<String>,
    /// Unix-время, секунды.
    pub received_at: i64,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub preview: String,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachment: bool,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Addr {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttachmentRow {
    pub blob_id: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// Content-ID для картинок в тексте (`cid:`).
    pub cid: Option<String>,
    pub inline: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BodyRow {
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub reply_to: Vec<Addr>,
    pub message_id: Option<String>,
    pub references: Vec<String>,
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<AttachmentRow>,
}

/// Blob'ы больше этого не кэшируются (их скачивают заново по требованию).
pub const MAX_CACHED_BLOB: usize = 16 * 1024 * 1024;

/// Адреса хранятся строками `имя\tадрес` через перевод строки: ни то, ни другое
/// не может содержать эти символы после разбора заголовков сервером.
fn addrs_to_db(list: &[Addr]) -> String {
    list.iter()
        .map(|a| format!("{}\t{}", a.name.replace(['\t', '\n'], " "), a.email.replace(['\t', '\n'], "")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn addrs_from_db(s: &str) -> Vec<Addr> {
    s.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(n, e)| Addr { name: n.to_string(), email: e.to_string() })
        .collect()
}

/// Кэш одноразовый: при смене версии схемы он пересоздаётся и заполняется заново.
const SCHEMA_VERSION: i64 = 3;

const DROP: &str = "
DROP TABLE IF EXISTS attachment;
DROP TABLE IF EXISTS blob;
DROP TABLE IF EXISTS body;
DROP TABLE IF EXISTS email_mailbox;
DROP TABLE IF EXISTS email;
DROP TABLE IF EXISTS mailbox;
DROP TABLE IF EXISTS sync_state;
";

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS mailbox (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    name TEXT NOT NULL,
    role TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    total INTEGER NOT NULL DEFAULT 0,
    unread INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS email (
    id TEXT PRIMARY KEY,
    thread_id TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    subject TEXT NOT NULL,
    from_name TEXT NOT NULL,
    from_addr TEXT NOT NULL,
    preview TEXT NOT NULL,
    seen INTEGER NOT NULL,
    flagged INTEGER NOT NULL,
    has_attachment INTEGER NOT NULL,
    size INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS email_mailbox (
    mailbox_id TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    email_id TEXT NOT NULL REFERENCES email(id) ON DELETE CASCADE,
    PRIMARY KEY (mailbox_id, received_at, email_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS email_mailbox_by_email ON email_mailbox(email_id);
CREATE TABLE IF NOT EXISTS body (
    email_id TEXT PRIMARY KEY REFERENCES email(id) ON DELETE CASCADE,
    recipients_to TEXT NOT NULL,
    recipients_cc TEXT NOT NULL,
    reply_to TEXT NOT NULL,
    message_id TEXT,
    refs TEXT NOT NULL,
    html TEXT,
    text TEXT
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS attachment (
    email_id TEXT NOT NULL REFERENCES email(id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    blob_id TEXT NOT NULL,
    name TEXT NOT NULL,
    mime TEXT NOT NULL,
    size INTEGER NOT NULL,
    cid TEXT,
    inline INTEGER NOT NULL,
    PRIMARY KEY (email_id, idx)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS blob (
    blob_id TEXT PRIMARY KEY,
    data BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS trusted_sender (
    email TEXT PRIMARY KEY
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS sync_state (
    kind TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;
";

pub struct Cache {
    db: Connection,
}

impl Cache {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Self> {
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            db.execute_batch(DROP)?;
            db.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        db.execute_batch(SCHEMA)?;
        // Небольшой кэш страниц: память важнее, чем лишнее чтение с SSD.
        db.pragma_update(None, "cache_size", -8 * 1024)?;
        Ok(Self { db })
    }

    pub fn replace_mailboxes(&mut self, rows: &[MailboxRow]) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM mailbox", [])?;
        {
            let mut st = tx.prepare(
                "INSERT INTO mailbox (id, parent_id, name, role, sort_order, total, unread) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )?;
            for m in rows {
                st.execute(params![m.id, m.parent_id, m.name, m.role, m.sort_order, m.total, m.unread])?;
            }
        }
        tx.commit()
    }

    pub fn mailboxes(&self) -> Result<Vec<MailboxRow>> {
        let mut st = self
            .db
            .prepare_cached("SELECT id, parent_id, name, role, sort_order, total, unread FROM mailbox ORDER BY sort_order, name")?;
        st.query_map([], |r| {
            Ok(MailboxRow {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                name: r.get(2)?,
                role: r.get(3)?,
                sort_order: r.get(4)?,
                total: r.get(5)?,
                unread: r.get(6)?,
            })
        })?
        .collect()
    }

    pub fn upsert_emails(&mut self, rows: &[EmailRow]) -> Result<()> {
        let tx = self.db.transaction()?;
        {
            let mut up = tx.prepare_cached(
                "INSERT INTO email (id, thread_id, received_at, subject, from_name, from_addr, preview, seen, flagged, has_attachment, size)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                 ON CONFLICT(id) DO UPDATE SET thread_id=excluded.thread_id, received_at=excluded.received_at,
                   subject=excluded.subject, from_name=excluded.from_name, from_addr=excluded.from_addr,
                   preview=excluded.preview, seen=excluded.seen, flagged=excluded.flagged,
                   has_attachment=excluded.has_attachment, size=excluded.size",
            )?;
            let mut del = tx.prepare_cached("DELETE FROM email_mailbox WHERE email_id = ?1")?;
            let mut link = tx.prepare_cached("INSERT INTO email_mailbox (mailbox_id, received_at, email_id) VALUES (?1,?2,?3)")?;
            for e in rows {
                up.execute(params![
                    e.id, e.thread_id, e.received_at, e.subject, e.from_name, e.from_addr, e.preview,
                    e.seen, e.flagged, e.has_attachment, e.size as i64
                ])?;
                del.execute([&e.id])?;
                for mb in &e.mailbox_ids {
                    link.execute(params![mb, e.received_at, e.id])?;
                }
            }
        }
        tx.commit()
    }

    pub fn delete_emails(&mut self, ids: &[String]) -> Result<()> {
        let tx = self.db.transaction()?;
        {
            let mut st = tx.prepare_cached("DELETE FROM email WHERE id = ?1")?;
            for id in ids {
                st.execute([id])?;
            }
        }
        tx.commit()
    }

    pub fn count(&self, mailbox_id: &str) -> Result<usize> {
        self.db
            .prepare_cached("SELECT count(*) FROM email_mailbox WHERE mailbox_id = ?1")?
            .query_row([mailbox_id], |r| r.get::<_, i64>(0))
            .map(|n| n as usize)
    }

    /// Окно списка: новые сверху. Для виртуального скролла.
    pub fn page(&self, mailbox_id: &str, offset: usize, limit: usize) -> Result<Vec<EmailRow>> {
        let mut st = self.db.prepare_cached(
            "SELECT e.id, e.thread_id, e.received_at, e.subject, e.from_name, e.from_addr, e.preview,
                    e.seen, e.flagged, e.has_attachment, e.size
             FROM email_mailbox m JOIN email e ON e.id = m.email_id
             WHERE m.mailbox_id = ?1
             ORDER BY m.received_at DESC, m.email_id DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        st.query_map(params![mailbox_id, limit as i64, offset as i64], |r| {
            Ok(EmailRow {
                id: r.get(0)?,
                thread_id: r.get(1)?,
                mailbox_ids: Vec::new(),
                received_at: r.get(2)?,
                subject: r.get(3)?,
                from_name: r.get(4)?,
                from_addr: r.get(5)?,
                preview: r.get(6)?,
                seen: r.get(7)?,
                flagged: r.get(8)?,
                has_attachment: r.get(9)?,
                size: r.get::<_, i64>(10)? as u64,
            })
        })?
        .collect()
    }

    pub fn set_flags(&self, id: &str, seen: bool, flagged: bool) -> Result<()> {
        self.db
            .prepare_cached("UPDATE email SET seen = ?2, flagged = ?3 WHERE id = ?1")?
            .execute(params![id, seen, flagged])
            .map(drop)
    }

    pub fn body(&self, id: &str) -> Result<Option<BodyRow>> {
        let body = self.body_only(id)?;
        body.map(|mut b| {
            b.attachments = self.attachments(id)?;
            Ok(b)
        })
        .transpose()
    }

    fn body_only(&self, id: &str) -> Result<Option<BodyRow>> {
        self.db
            .prepare_cached(
                "SELECT recipients_to, recipients_cc, reply_to, message_id, refs, html, text FROM body WHERE email_id = ?1",
            )?
            .query_row([id], |r| {
                Ok(BodyRow {
                    to: addrs_from_db(&r.get::<_, String>(0)?),
                    cc: addrs_from_db(&r.get::<_, String>(1)?),
                    reply_to: addrs_from_db(&r.get::<_, String>(2)?),
                    message_id: r.get(3)?,
                    references: r.get::<_, String>(4)?.split_whitespace().map(str::to_string).collect(),
                    html: r.get(5)?,
                    text: r.get(6)?,
                    attachments: Vec::new(),
                })
            })
            .optional()
    }

    pub fn put_body(&mut self, id: &str, b: &BodyRow) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.prepare_cached("DELETE FROM attachment WHERE email_id = ?1")?.execute([id])?;
        {
            let mut st = tx.prepare_cached(
                "INSERT INTO attachment (email_id, idx, blob_id, name, mime, size, cid, inline) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )?;
            for (i, a) in b.attachments.iter().enumerate() {
                st.execute(params![id, i as i64, a.blob_id, a.name, a.mime, a.size as i64, a.cid, a.inline])?;
            }
        }
        tx
            .prepare_cached(
                "INSERT OR REPLACE INTO body (email_id, recipients_to, recipients_cc, reply_to, message_id, refs, html, text)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )?
            .execute(params![
                id,
                addrs_to_db(&b.to),
                addrs_to_db(&b.cc),
                addrs_to_db(&b.reply_to),
                b.message_id,
                b.references.join(" "),
                b.html,
                b.text
            ])?;
        tx.commit()
    }

    pub fn attachments(&self, email_id: &str) -> Result<Vec<AttachmentRow>> {
        self.db
            .prepare_cached("SELECT blob_id, name, mime, size, cid, inline FROM attachment WHERE email_id = ?1 ORDER BY idx")?
            .query_map([email_id], |r| {
                Ok(AttachmentRow {
                    blob_id: r.get(0)?,
                    name: r.get(1)?,
                    mime: r.get(2)?,
                    size: r.get::<_, i64>(3)? as u64,
                    cid: r.get(4)?,
                    inline: r.get(5)?,
                })
            })?
            .collect()
    }

    pub fn blob(&self, blob_id: &str) -> Result<Option<Vec<u8>>> {
        self.db.prepare_cached("SELECT data FROM blob WHERE blob_id = ?1")?.query_row([blob_id], |r| r.get(0)).optional()
    }

    /// Сохраняет blob, если он не больше [`MAX_CACHED_BLOB`].
    pub fn put_blob(&self, blob_id: &str, data: &[u8]) -> Result<()> {
        if data.len() > MAX_CACHED_BLOB {
            return Ok(());
        }
        self.db
            .prepare_cached("INSERT OR REPLACE INTO blob (blob_id, data) VALUES (?1, ?2)")?
            .execute(params![blob_id, data])
            .map(drop)
    }

    pub fn is_trusted(&self, email: &str) -> Result<bool> {
        self.db
            .prepare_cached("SELECT 1 FROM trusted_sender WHERE email = ?1")?
            .exists([email.to_lowercase()])
    }

    pub fn trust(&self, email: &str) -> Result<()> {
        self.db
            .prepare_cached("INSERT OR IGNORE INTO trusted_sender (email) VALUES (?1)")?
            .execute([email.to_lowercase()])
            .map(drop)
    }

    pub fn state(&self, kind: &str) -> Result<Option<String>> {
        self.db
            .prepare_cached("SELECT value FROM sync_state WHERE kind = ?1")?
            .query_row([kind], |r| r.get(0))
            .optional()
    }

    pub fn set_state(&self, kind: &str, value: &str) -> Result<()> {
        self.db
            .prepare_cached("INSERT OR REPLACE INTO sync_state (kind, value) VALUES (?1, ?2)")?
            .execute([kind, value])
            .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email(id: &str, at: i64, mbs: &[&str]) -> EmailRow {
        EmailRow {
            id: id.into(),
            thread_id: id.into(),
            mailbox_ids: mbs.iter().map(|s| s.to_string()).collect(),
            received_at: at,
            subject: format!("s{id}"),
            from_name: String::new(),
            from_addr: String::new(),
            preview: String::new(),
            seen: false,
            flagged: false,
            has_attachment: false,
            size: 1,
        }
    }

    #[test]
    fn paging_and_moves() {
        let mut c = Cache::open_in_memory().unwrap();
        c.upsert_emails(&[email("a", 1, &["in"]), email("b", 3, &["in"]), email("c", 2, &["in", "x"])]).unwrap();
        let ids: Vec<_> = c.page("in", 0, 10).unwrap().into_iter().map(|e| e.id).collect();
        assert_eq!(ids, ["b", "c", "a"]);
        assert_eq!(c.page("in", 1, 1).unwrap()[0].id, "c");
        // Перемещение: письмо пропадает из старого ящика.
        c.upsert_emails(&[email("c", 2, &["x"])]).unwrap();
        assert_eq!(c.count("in").unwrap(), 2);
        c.delete_emails(&["c".into()]).unwrap();
        assert_eq!(c.count("x").unwrap(), 0);
    }

    #[test]
    fn bodies_and_state() {
        let mut c = Cache::open_in_memory().unwrap();
        c.upsert_emails(&[email("a", 1, &["in"])]).unwrap();
        assert!(c.body("a").unwrap().is_none());
        let b = BodyRow {
            text: Some("hi".into()),
            to: vec![Addr { name: "Иванов, Иван".into(), email: "i@x.ru".into() }, Addr { name: String::new(), email: "p@x.ru".into() }],
            message_id: Some("m@x".into()),
            references: vec!["a@x".into(), "b@x".into()],
            ..Default::default()
        };
        let mut b = b;
        b.attachments = vec![AttachmentRow { blob_id: "B1".into(), name: "x.png".into(), mime: "image/png".into(), size: 3, cid: Some("c1".into()), inline: true }];
        c.put_body("a", &b).unwrap();
        c.put_blob("B1", b"png").unwrap();
        assert_eq!(c.blob("B1").unwrap().as_deref(), Some(&b"png"[..]));
        c.trust("Ivan@X.ru").unwrap();
        assert!(c.is_trusted("ivan@x.ru").unwrap());
        assert_eq!(c.body("a").unwrap(), Some(b));
        c.set_state("Email", "s1").unwrap();
        assert_eq!(c.state("Email").unwrap().as_deref(), Some("s1"));
    }
}
