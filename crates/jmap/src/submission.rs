//! Отправка (RFC 8621 §7): письмо собирается на клиенте целиком, загружается
//! blob'ом, импортируется в «Черновики» и отправляется через EmailSubmission;
//! после успеха сервер сам переносит его в «Отправленные».

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::mail::{GetResponse, USING, inv, set_error};
use crate::{CORE, Client, Error, MAIL, Result, SUBMISSION, take};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub text_signature: String,
}

/// SMTP-конверт: отправитель и все получатели, включая скрытых.
#[derive(Debug, Clone)]
pub struct Envelope {
    pub mail_from: String,
    pub rcpt_to: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Uploaded {
    blob_id: String,
}

impl Client {
    pub async fn identities(&self) -> Result<Vec<Identity>> {
        let acc = self.account_for(SUBMISSION)?;
        let mut r = self
            .call(&[CORE, MAIL, SUBMISSION], &[inv("Identity/get", json!({ "accountId": acc, "ids": null }), "i")])
            .await?;
        let g: GetResponse<Identity> = take(&mut r, "i")?;
        Ok(g.list)
    }

    pub async fn upload(&self, account: &str, data: Vec<u8>, mime: &str) -> Result<String> {
        let url = self.session().upload_url.replace("{accountId}", account);
        let up: Uploaded = self
            .authorized(self.http().post(url))
            .header(reqwest::header::CONTENT_TYPE, mime)
            .body(data)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(up.blob_id)
    }

    /// Сохранить черновик; прежняя версия (`replace`) удаляется тем же запросом.
    pub async fn save_draft(&self, raw: Vec<u8>, drafts: &str, replace: Option<&str>) -> Result<String> {
        let acc = self.mail_account()?.to_string();
        let blob = self.upload(&acc, raw, "message/rfc822").await?;
        let mut calls = vec![import_call(&acc, &blob, drafts)];
        if let Some(old) = replace {
            calls.push(inv("Email/set", json!({ "accountId": acc, "destroy": [old] }), "d"));
        }
        let mut r = self.call(USING, &calls).await?;
        imported_id(&mut r)
    }

    /// Отправить собранное письмо. Возвращает id письма (оно окажется в `sent`).
    pub async fn send(&self, raw: Vec<u8>, identity: &str, envelope: &Envelope, drafts: &str, sent: &str) -> Result<String> {
        let acc = self.mail_account()?.to_string();
        let blob = self.upload(&acc, raw, "message/rfc822").await?;
        let rcpt: Vec<Value> = envelope.rcpt_to.iter().map(|e| json!({ "email": e })).collect();
        let mut on_success = Map::new();
        on_success.insert(format!("mailboxIds/{drafts}"), Value::Null);
        on_success.insert(format!("mailboxIds/{sent}"), Value::Bool(true));
        on_success.insert("keywords/$draft".into(), Value::Null);
        let calls = [
            import_call(&acc, &blob, drafts),
            inv(
                "EmailSubmission/set",
                json!({
                    "accountId": acc,
                    "create": { "sub": {
                        "identityId": identity,
                        "emailId": "#draft",
                        "envelope": { "mailFrom": { "email": envelope.mail_from }, "rcptTo": rcpt },
                    }},
                    "onSuccessUpdateEmail": { "#sub": on_success },
                }),
                "s",
            ),
        ];
        let mut r = self.call(&[CORE, MAIL, SUBMISSION], &calls).await?;
        let id = imported_id(&mut r)?;
        let v: Value = take(&mut r, "s")?;
        set_error(&v, "notCreated", "sub", "EmailSubmission/set")?;
        Ok(id)
    }
}

fn import_call(acc: &str, blob: &str, drafts: &str) -> crate::Invocation {
    inv(
        "Email/import",
        json!({
            "accountId": acc,
            "emails": { "draft": {
                "blobId": blob,
                "mailboxIds": { drafts: true },
                "keywords": { "$draft": true, "$seen": true },
            }},
        }),
        "i",
    )
}

fn imported_id(r: &mut [crate::Invocation]) -> Result<String> {
    let v: Value = take(r, "i")?;
    set_error(&v, "notCreated", "draft", "Email/import")?;
    v["created"]["draft"]["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Protocol("Email/import не вернул id".into()))
}
