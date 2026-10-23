//! Таблица категорий на сервере (S5-02б, решение владельца №4): файл JMAP FileNode `.postman/categories.json`
//! в аккаунте — свой у пользователя, свой у каждого общего ящика. Таблица общая: её видят все клиенты учётной
//! записи (и все участники общего ящика).
//!
//! - **Чтение** — при входе (`sync`, для своего аккаунта и общих ящиков) и по push, когда у аккаунта изменились
//!   файлы (`FileNode` в `StateChange`): папка по имени в корне, файл по имени в папке, содержимое — blob.
//! - **Запись** — сведение и запись с проверкой: прочитать таблицу и состояние FileNode, свести свою с ней
//!   ([`Categories::merge`] — у каждой категории побеждает поздняя правка), загрузить blob и заменить им
//!   содержимое файла с `ifInState`. Состояние изменилось (другой клиент успел записать) — всё заново, до
//!   [`ATTEMPTS`] раз. Папку и файл создаёт первая запись; созданные одновременно двумя клиентами — `alreadyExists`,
//!   тогда тоже заново.
//! - **Нет FileNode** (сервер не объявил или аккаунт без файлов) — [`Reply::Unsupported`]: таблица живёт только в
//!   локальном файле (`labels::LocalFile`). Сервер недоступен — [`Reply::Failed`], правка ждёт (`Book::pending`).
//! - **Снять категорию с писем** — только явным выбором при удалении ([`Request::Purge`]): по умолчанию удалённая
//!   категория скрыта, ключевое слово у писем остаётся.
//!
//! Файл виден пользователю в «Файлах» (WebDAV, `/dav/file/<ящик>/.postman/`): удалит — таблица вернётся к
//! умолчанию, следующая правка создаст файл заново.
//!
//! Здесь — договор (запросы, ответы, формат файла); FileNode — у плагина JMAP (`postman-backend-jmap`, ADR 0004).

use serde::{Deserialize, Serialize};

use crate::labels::{Categories, Category};

/// Папка Postman в корне файлов аккаунта.
pub const FOLDER: &str = ".postman";
pub const FILE: &str = "categories.json";
/// Формат файла: растёт при несовместимых изменениях; файл новее — не читается и не перезаписывается.
pub const FILE_VERSION: u32 = 1;
/// Сколько раз пробовать записать, если состояние сервера менялось.
pub const ATTEMPTS: usize = 5;

/// Содержимое файла на сервере.
#[derive(Debug, Serialize, Deserialize)]
struct TableFile {
    version: u32,
    categories: Vec<Category>,
}

/// Аккаунт — как в кэше: `None` — свой, иначе id общего ящика (он же префикс ключей кэша).
pub type Scope = Option<String>;

pub enum Request {
    /// Перечитать таблицу аккаунта → `Loaded`.
    Load { account: Scope },
    /// Свести таблицу с серверной и записать → `Saved` (как она теперь на сервере), `Unsupported` или `Failed`.
    Save { account: Scope, table: Categories },
    /// Снять ключевое слово со всех писем аккаунта (удаление категории с явным выбором «и с писем») → `Purged`
    /// или `PurgeFailed`.
    Purge { account: Scope, keyword: String, ticket: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// Таблица аккаунта на сервере; `None` — файла нет (таблица по умолчанию).
    Loaded {
        account: Scope,
        table: Option<Categories>,
    },
    Saved {
        account: Scope,
        table: Categories,
    },
    /// У аккаунта нет файлов (FileNode): таблица — только локальная.
    Unsupported {
        account: Scope,
    },
    Failed {
        account: Scope,
        error: String,
    },
    /// Слово снято с `count` писем.
    Purged {
        account: Scope,
        keyword: String,
        ticket: u64,
        count: usize,
    },
    PurgeFailed {
        ticket: u64,
        error: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Server(#[from] crate::model::Error),
    #[error("categories file: {0}")]
    Json(#[from] serde_json::Error),
    #[error("categories file version {0} is newer than this Postman")]
    Version(u32),
    #[error("account has no files (FileNode)")]
    Unsupported,
    #[error("categories file kept changing: {ATTEMPTS} attempts")]
    Busy,
}

/// Таблица → файл.
pub fn encode(table: &Categories) -> Result<Vec<u8>, Error> {
    Ok(serde_json::to_vec_pretty(&TableFile { version: FILE_VERSION, categories: table.list.clone() })?)
}

/// Файл → таблица.
pub fn decode(data: &[u8]) -> Result<Categories, Error> {
    #[derive(Deserialize)]
    struct Head {
        #[serde(default)]
        version: u32,
    }
    let Head { version } = serde_json::from_slice(data)?;
    if version > FILE_VERSION {
        return Err(Error::Version(version));
    }
    let file: TableFile = serde_json::from_slice(data)?;
    Ok(Categories { list: file.categories })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_round_trip_and_newer_version() {
        let mut t = Categories::defaults(|id| format!("Категория {id}"));
        t.delete("pm.cat.team", 5).unwrap();
        let data = encode(&t).unwrap();
        let text = String::from_utf8(data.clone()).unwrap();
        assert!(text.contains("\"version\": 1") && text.contains("\"color\": \"blue\""), "{text}");
        assert_eq!(decode(&data).unwrap(), t);
        assert!(matches!(decode(br#"{"version": 2, "categories": []}"#), Err(Error::Version(2))));
        assert!(matches!(decode(b"[]"), Err(Error::Json(_))));
        // Поля из новой версии того же формата не мешают.
        let extra = br#"{"version": 1, "categories": [{"keyword": "pm.cat.x", "name": "X", "color": "navy", "icon": "star"}], "x": 1}"#;
        let got = decode(extra).unwrap();
        assert_eq!((got.list[0].key, got.list[0].changed, got.list[0].hidden), (None, 0, false));
    }
}
