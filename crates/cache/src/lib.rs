//! Локальное хранилище клиента на redb (встраиваемое ключ-значение, чистый Rust).
//!
//! Главная копия почты — на сервере; здесь то, что нужно для мгновенного старта,
//! чтения без сети и списка без держания ящика в памяти. Порядок писем в папке
//! задаёт составной ключ `(папка, -дата, id)`: страница списка — чтение диапазона.
//! Хранилище одноразовое: при смене формата оно пересоздаётся и заполняется синхронизацией.

use std::path::Path;
use std::sync::Arc;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

macro_rules! from_err {
    ($($t:ty),*) => {$(
        impl From<$t> for Error {
            fn from(e: $t) -> Self {
                Error(e.to_string())
            }
        }
    )*};
}
from_err!(
    redb::Error,
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError,
    postcard::Error,
    std::io::Error
);

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailboxRow {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub role: Option<String>,
    pub sort_order: u32,
    pub total: u32,
    pub unread: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Addr {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AttachmentRow {
    pub blob_id: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// Content-ID для картинок в тексте (`cid:`).
    pub cid: Option<String>,
    pub inline: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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

/// Меняется при любом изменении формата значений или ключей.
const FORMAT_VERSION: u64 = 1;

const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
const MAILBOX: TableDefinition<&str, &[u8]> = TableDefinition::new("mailbox");
const EMAIL: TableDefinition<&str, &[u8]> = TableDefinition::new("email");
/// (папка, -дата, id) → () — порядок списка.
const BY_MAILBOX: TableDefinition<(&str, i64, &str), ()> = TableDefinition::new("by_mailbox");
/// Число писем в папке — чтобы не считать диапазон при каждом обновлении списка.
const COUNT: TableDefinition<&str, u64> = TableDefinition::new("count");
const BODY: TableDefinition<&str, &[u8]> = TableDefinition::new("body");
const BLOB: TableDefinition<&str, &[u8]> = TableDefinition::new("blob");
const TRUSTED: TableDefinition<&str, ()> = TableDefinition::new("trusted_sender");
const STATE: TableDefinition<&str, &str> = TableDefinition::new("sync_state");

/// Страничный кэш redb: памяти важнее, чем лишнее чтение с SSD.
const CACHE_BYTES: usize = 16 * 1024 * 1024;

fn enc<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    Ok(postcard::to_stdvec(v)?)
}

fn dec<T: DeserializeOwned>(b: &[u8]) -> Result<T> {
    Ok(postcard::from_bytes(b)?)
}

/// Верхняя граница диапазона ключей папки.
const MAX_ID: &str = "\u{10ffff}";

/// Дешёвый в клонировании дескриптор: один `Database` на процесс, общий для UI и синхронизации.
#[derive(Clone)]
pub struct Cache {
    db: Arc<Database>,
}

impl Cache {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let open = || Database::builder().set_cache_size(CACHE_BYTES).create(path);
        let db = match open() {
            Ok(db) => db,
            // Файл другого формата (например, старый кэш SQLite): пересоздаём.
            Err(redb::DatabaseError::Storage(_)) => {
                std::fs::remove_file(path)?;
                open()?
            }
            Err(e) => return Err(e.into()),
        };
        let mut c = Self { db: Arc::new(db) };
        if c.version()? != Some(FORMAT_VERSION) {
            drop(c);
            std::fs::remove_file(path)?;
            c = Self { db: Arc::new(open()?) };
        }
        c.init()?;
        Ok(c)
    }

    pub fn open_in_memory() -> Result<Self> {
        let db = Database::builder().create_with_backend(redb::backends::InMemoryBackend::new())?;
        let c = Self { db: Arc::new(db) };
        c.init()?;
        Ok(c)
    }

    fn version(&self) -> Result<Option<u64>> {
        let tx = self.db.begin_read()?;
        match tx.open_table(META) {
            Ok(t) => Ok(t.get("version")?.map(|v| v.value())),
            // Пустая новая база — тоже «наш» формат.
            Err(redb::TableError::TableDoesNotExist(_)) => Ok(tx.list_tables()?.next().is_none().then_some(FORMAT_VERSION)),
            Err(e) => Err(e.into()),
        }
    }

    fn init(&self) -> Result<()> {
        let tx = self.db.begin_write()?;
        tx.open_table(META)?.insert("version", FORMAT_VERSION)?;
        // Создаём все таблицы, чтобы чтение не спотыкалось об их отсутствие.
        tx.open_table(MAILBOX)?;
        tx.open_table(EMAIL)?;
        tx.open_table(BY_MAILBOX)?;
        tx.open_table(COUNT)?;
        tx.open_table(BODY)?;
        tx.open_table(BLOB)?;
        tx.open_table(TRUSTED)?;
        tx.open_table(STATE)?;
        tx.commit()?;
        Ok(())
    }

    pub fn replace_mailboxes(&self, rows: &[MailboxRow]) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            let mut t = tx.open_table(MAILBOX)?;
            t.retain(|_, _| false)?;
            for m in rows {
                t.insert(m.id.as_str(), enc(m)?.as_slice())?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn mailboxes(&self) -> Result<Vec<MailboxRow>> {
        let tx = self.db.begin_read()?;
        let t = tx.open_table(MAILBOX)?;
        let mut out: Vec<MailboxRow> = t.iter()?.map(|r| dec(r?.1.value())).collect::<Result<_>>()?;
        out.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then_with(|| a.name.cmp(&b.name)));
        Ok(out)
    }

    pub fn upsert_emails(&self, rows: &[EmailRow]) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            let mut email = tx.open_table(EMAIL)?;
            let mut by = tx.open_table(BY_MAILBOX)?;
            let mut count = tx.open_table(COUNT)?;
            for e in rows {
                let old: Option<EmailRow> = email.get(e.id.as_str())?.map(|v| dec(v.value())).transpose()?;
                if let Some(old) = &old {
                    for mb in &old.mailbox_ids {
                        if by.remove((mb.as_str(), -old.received_at, old.id.as_str()))?.is_some() {
                            add(&mut count, mb, -1)?;
                        }
                    }
                }
                for mb in &e.mailbox_ids {
                    if by.insert((mb.as_str(), -e.received_at, e.id.as_str()), ())?.is_none() {
                        add(&mut count, mb, 1)?;
                    }
                }
                email.insert(e.id.as_str(), enc(e)?.as_slice())?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_emails(&self, ids: &[String]) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            let mut email = tx.open_table(EMAIL)?;
            let mut by = tx.open_table(BY_MAILBOX)?;
            let mut count = tx.open_table(COUNT)?;
            let mut body = tx.open_table(BODY)?;
            for id in ids {
                let old: Option<EmailRow> = email.remove(id.as_str())?.map(|v| dec(v.value())).transpose()?;
                let Some(old) = old else { continue };
                for mb in &old.mailbox_ids {
                    if by.remove((mb.as_str(), -old.received_at, id.as_str()))?.is_some() {
                        add(&mut count, mb, -1)?;
                    }
                }
                body.remove(id.as_str())?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn count(&self, mailbox_id: &str) -> Result<usize> {
        let tx = self.db.begin_read()?;
        Ok(tx.open_table(COUNT)?.get(mailbox_id)?.map_or(0, |v| v.value() as usize))
    }

    /// Окно списка: новые сверху. Для виртуального скролла.
    pub fn page(&self, mailbox_id: &str, offset: usize, limit: usize) -> Result<Vec<EmailRow>> {
        let tx = self.db.begin_read()?;
        let by = tx.open_table(BY_MAILBOX)?;
        let email = tx.open_table(EMAIL)?;
        let mut out = Vec::with_capacity(limit);
        for k in by.range((mailbox_id, i64::MIN, "")..=(mailbox_id, i64::MAX, MAX_ID))?.skip(offset).take(limit) {
            let (k, _) = k?;
            if let Some(v) = email.get(k.value().2)? {
                out.push(dec(v.value())?);
            }
        }
        Ok(out)
    }

    /// Строки по id в заданном порядке (результаты поиска); отсутствующие пропускаются.
    pub fn rows_by_ids(&self, ids: &[String]) -> Result<Vec<EmailRow>> {
        let tx = self.db.begin_read()?;
        let email = tx.open_table(EMAIL)?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(v) = email.get(id.as_str())? {
                out.push(dec(v.value())?);
            }
        }
        Ok(out)
    }

    /// Оставить из `ids` только письма ящика, сохранив порядок.
    pub fn filter_mailbox(&self, ids: &[String], mailbox_id: &str) -> Result<Vec<String>> {
        Ok(self
            .rows_by_ids(ids)?
            .into_iter()
            .filter(|e| e.mailbox_ids.iter().any(|m| m == mailbox_id))
            .map(|e| e.id)
            .collect())
    }

    /// Все письма порциями (для первичного построения поискового индекса).
    pub fn all_rows(&self, offset: usize, limit: usize) -> Result<Vec<EmailRow>> {
        let tx = self.db.begin_read()?;
        let email = tx.open_table(EMAIL)?;
        email.iter()?.skip(offset).take(limit).map(|r| dec(r?.1.value())).collect()
    }

    /// Самое старое письмо ящика в кэше — якорь для догрузки следующей страницы.
    pub fn oldest(&self, mailbox_id: &str) -> Result<Option<String>> {
        let tx = self.db.begin_read()?;
        let by = tx.open_table(BY_MAILBOX)?;
        let last = by.range((mailbox_id, i64::MIN, "")..=(mailbox_id, i64::MAX, MAX_ID))?.next_back().transpose()?;
        Ok(last.map(|(k, _)| k.value().2.to_string()))
    }

    /// Простой поиск по кэшу — запасной путь, если поисковый индекс недоступен.
    pub fn search(&self, query: &str, mailbox_id: Option<&str>, limit: usize) -> Result<Vec<String>> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let tx = self.db.begin_read()?;
        let email = tx.open_table(EMAIL)?;
        let mut hits: Vec<(i64, String)> = Vec::new();
        for r in email.iter()? {
            let e: EmailRow = dec(r?.1.value())?;
            if mailbox_id.is_some_and(|mb| !e.mailbox_ids.iter().any(|m| m == mb)) {
                continue;
            }
            let hay = format!("{} {} {} {}", e.subject, e.from_name, e.from_addr, e.preview).to_lowercase();
            if words.iter().all(|w| hay.contains(w.as_str())) {
                hits.push((e.received_at, e.id));
            }
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
        Ok(hits.into_iter().take(limit).map(|(_, id)| id).collect())
    }

    pub fn set_flags(&self, id: &str, seen: bool, flagged: bool) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            let mut email = tx.open_table(EMAIL)?;
            let row: Option<EmailRow> = email.get(id)?.map(|v| dec(v.value())).transpose()?;
            if let Some(mut row) = row {
                row.seen = seen;
                row.flagged = flagged;
                email.insert(id, enc(&row)?.as_slice())?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn body(&self, id: &str) -> Result<Option<BodyRow>> {
        let tx = self.db.begin_read()?;
        tx.open_table(BODY)?.get(id)?.map(|v| dec(v.value())).transpose()
    }

    pub fn put_body(&self, id: &str, b: &BodyRow) -> Result<()> {
        self.put(BODY, id, &enc(b)?)
    }

    pub fn attachments(&self, email_id: &str) -> Result<Vec<AttachmentRow>> {
        Ok(self.body(email_id)?.map(|b| b.attachments).unwrap_or_default())
    }

    pub fn blob(&self, blob_id: &str) -> Result<Option<Vec<u8>>> {
        let tx = self.db.begin_read()?;
        Ok(tx.open_table(BLOB)?.get(blob_id)?.map(|v| v.value().to_vec()))
    }

    /// Сохраняет blob, если он не больше [`MAX_CACHED_BLOB`].
    pub fn put_blob(&self, blob_id: &str, data: &[u8]) -> Result<()> {
        if data.len() > MAX_CACHED_BLOB {
            return Ok(());
        }
        self.put(BLOB, blob_id, data)
    }

    pub fn is_trusted(&self, email: &str) -> Result<bool> {
        let tx = self.db.begin_read()?;
        Ok(tx.open_table(TRUSTED)?.get(email.to_lowercase().as_str())?.is_some())
    }

    pub fn trust(&self, email: &str) -> Result<()> {
        let tx = self.db.begin_write()?;
        tx.open_table(TRUSTED)?.insert(email.to_lowercase().as_str(), ())?;
        tx.commit()?;
        Ok(())
    }

    pub fn state(&self, kind: &str) -> Result<Option<String>> {
        let tx = self.db.begin_read()?;
        Ok(tx.open_table(STATE)?.get(kind)?.map(|v| v.value().to_string()))
    }

    pub fn set_state(&self, kind: &str, value: &str) -> Result<()> {
        let tx = self.db.begin_write()?;
        tx.open_table(STATE)?.insert(kind, value)?;
        tx.commit()?;
        Ok(())
    }

    fn put(&self, table: TableDefinition<&str, &[u8]>, key: &str, value: &[u8]) -> Result<()> {
        let tx = self.db.begin_write()?;
        tx.open_table(table)?.insert(key, value)?;
        tx.commit()?;
        Ok(())
    }
}

fn add(count: &mut redb::Table<&str, u64>, mailbox: &str, delta: i64) -> Result<()> {
    let cur = count.get(mailbox)?.map_or(0, |v| v.value());
    count.insert(mailbox, cur.saturating_add_signed(delta))?;
    Ok(())
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

    fn ids(rows: Vec<EmailRow>) -> Vec<String> {
        rows.into_iter().map(|e| e.id).collect()
    }

    #[test]
    fn paging_and_moves() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_emails(&[email("a", 1, &["in"]), email("b", 3, &["in"]), email("c", 2, &["in", "x"])]).unwrap();
        assert_eq!(ids(c.page("in", 0, 10).unwrap()), ["b", "c", "a"]);
        assert_eq!(c.page("in", 1, 1).unwrap()[0].id, "c");
        assert_eq!(c.oldest("in").unwrap().as_deref(), Some("a"));
        assert_eq!(ids(c.rows_by_ids(&["c".into(), "zz".into(), "a".into()]).unwrap()), ["c", "a"]);
        // Перемещение: письмо пропадает из старого ящика; повторная запись не удваивает счётчик.
        c.upsert_emails(&[email("c", 2, &["x"])]).unwrap();
        c.upsert_emails(&[email("c", 2, &["x"])]).unwrap();
        assert_eq!(c.count("in").unwrap(), 2);
        assert_eq!(c.count("x").unwrap(), 1);
        assert_eq!(c.filter_mailbox(&["a".into(), "c".into()], "x").unwrap(), ["c"]);
        c.delete_emails(&["c".into()]).unwrap();
        assert_eq!(c.count("x").unwrap(), 0);
        assert_eq!(c.all_rows(0, 10).unwrap().len(), 2);
    }

    #[test]
    fn mailboxes_flags_and_search() {
        let c = Cache::open_in_memory().unwrap();
        let mb = |id: &str, order| MailboxRow {
            id: id.into(),
            parent_id: None,
            name: id.into(),
            role: None,
            sort_order: order,
            total: 0,
            unread: 0,
        };
        c.replace_mailboxes(&[mb("b", 2), mb("a", 1)]).unwrap();
        c.replace_mailboxes(&[mb("z", 2), mb("a", 1)]).unwrap();
        assert_eq!(c.mailboxes().unwrap().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "z"]);
        let mut e = email("a", 1, &["in"]);
        e.subject = "Отчёт за КВАРТАЛ".into();
        c.upsert_emails(&[e, email("b", 2, &["x"])]).unwrap();
        c.set_flags("a", true, true).unwrap();
        let row = &c.page("in", 0, 1).unwrap()[0];
        assert!(row.seen && row.flagged);
        assert_eq!(c.search("квартал", None, 10).unwrap(), ["a"]);
        assert!(c.search("квартал", Some("x"), 10).unwrap().is_empty());
    }

    #[test]
    fn bodies_blobs_state() {
        let c = Cache::open_in_memory().unwrap();
        c.upsert_emails(&[email("a", 1, &["in"])]).unwrap();
        assert!(c.body("a").unwrap().is_none());
        let b = BodyRow {
            text: Some("hi".into()),
            to: vec![Addr { name: "Иванов, Иван".into(), email: "i@x.ru".into() }],
            message_id: Some("m@x".into()),
            references: vec!["a@x".into(), "b@x".into()],
            attachments: vec![AttachmentRow {
                blob_id: "B1".into(),
                name: "x.png".into(),
                mime: "image/png".into(),
                size: 3,
                cid: Some("c1".into()),
                inline: true,
            }],
            ..Default::default()
        };
        c.put_body("a", &b).unwrap();
        assert_eq!(c.body("a").unwrap(), Some(b));
        assert_eq!(c.attachments("a").unwrap().len(), 1);
        c.put_blob("B1", b"png").unwrap();
        assert_eq!(c.blob("B1").unwrap().as_deref(), Some(&b"png"[..]));
        c.trust("Ivan@X.ru").unwrap();
        assert!(c.is_trusted("ivan@x.ru").unwrap());
        c.set_state("Email", "s1").unwrap();
        assert_eq!(c.state("Email").unwrap().as_deref(), Some("s1"));
        c.delete_emails(&["a".into()]).unwrap();
        assert!(c.body("a").unwrap().is_none());
    }

    #[test]
    fn reopen_and_replace_foreign_file() {
        let dir = std::env::temp_dir().join(format!("postman-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.redb");
        std::fs::write(&path, b"SQLite format 3\0 not redb").unwrap();
        {
            let c = Cache::open(&path).unwrap();
            c.upsert_emails(&[email("a", 1, &["in"])]).unwrap();
        }
        assert_eq!(Cache::open(&path).unwrap().count("in").unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
