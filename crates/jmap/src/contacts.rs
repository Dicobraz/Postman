//! JMAP for Contacts (RFC 9610): карточки JSContact (RFC 9553) → (имя, адреса).

use serde_json::{Value, json};

use crate::mail::{GetResponse, inv};
use crate::{CONTACTS, CORE, Client, Result, take};

/// Имя и адреса одной карточки.
#[derive(Debug, Clone, PartialEq)]
pub struct ContactCard {
    pub id: String,
    pub name: String,
    pub emails: Vec<String>,
}

impl Client {
    /// Все карточки личных адресных книг.
    pub async fn contact_cards(&self) -> Result<Vec<ContactCard>> {
        let acc = self.account_for(CONTACTS)?;
        let args = json!({ "accountId": acc, "ids": null, "properties": ["id", "name", "emails"] });
        let mut r = self.call(&[CORE, CONTACTS], &[inv("ContactCard/get", args, "c")]).await?;
        let g: GetResponse<Value> = take(&mut r, "c")?;
        Ok(g.list.iter().map(card).collect())
    }
}

fn card(v: &Value) -> ContactCard {
    let name = &v["name"];
    let full = name["full"].as_str().map(str::to_string).unwrap_or_else(|| {
        // Нет `full` — собираем из компонентов: фамилия, имя, отчество.
        let part = |kind: &str| -> Vec<&str> {
            name["components"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["kind"] == kind)
                .filter_map(|c| c["value"].as_str())
                .collect()
        };
        [part("surname"), part("given"), part("given2")].concat().join(" ")
    });
    let emails = v["emails"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
        .filter_map(|e| e["address"].as_str().map(str::to_string))
        .collect();
    ContactCard { id: v["id"].as_str().unwrap_or_default().to_string(), name: full, emails }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jscontact_card() {
        let c = card(&json!({
            "id": "c1",
            "name": { "components": [{ "kind": "given", "value": "Мария" }, { "kind": "surname", "value": "Иванова" }] },
            "emails": { "e1": { "address": "m@x.ru" }, "e2": { "address": "maria@home.ru" } }
        }));
        assert_eq!(c.name, "Иванова Мария");
        assert_eq!(c.emails.len(), 2);
        let c = card(&json!({ "id": "c2", "name": { "full": "ООО Ромашка" }, "emails": {} }));
        assert_eq!(c.name, "ООО Ромашка");
    }
}
