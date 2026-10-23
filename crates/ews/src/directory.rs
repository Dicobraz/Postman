//! Справочник (GAL): `ResolveNames` по строке, только Active Directory, с данными контакта — имя, адрес,
//! должность, отдел, телефон. Подсказки адресов и карточка человека (S7-08).

use crate::soap;
use crate::xml::{El, esc};
use crate::{Client, Result};

/// Человек или список рассылки из справочника.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Person {
    pub name: String,
    pub email: String,
    pub title: String,
    pub department: String,
    /// Рабочий, иначе мобильный, иначе первый из данных.
    pub phone: String,
    pub company: String,
    pub office: String,
    /// Список рассылки (`PublicDL`), не человек.
    pub group: bool,
}

pub(crate) fn request(query: &str) -> String {
    format!(
        r#"<m:ResolveNames ReturnFullContactData="true" SearchScope="ActiveDirectory"><m:UnresolvedEntry>{}</m:UnresolvedEntry></m:ResolveNames>"#,
        esc(query)
    )
}

fn smtp(addr: &str) -> String {
    addr.strip_prefix("SMTP:").or_else(|| addr.strip_prefix("smtp:")).unwrap_or(addr).to_owned()
}

fn person(r: &El) -> Option<Person> {
    let mailbox = r.child("Mailbox")?;
    let contact = r.child("Contact");
    let c = |path: &[&str]| contact.map(|c| c.str_at(path)).unwrap_or_default();
    // У ящика по SMTP адрес — `EmailAddress`; иначе (`EX`) — первый SMTP из данных контакта.
    let email = match mailbox.text_at(&["RoutingType"]) {
        Some("SMTP") | None => mailbox.str_at(&["EmailAddress"]),
        Some(_) => contact
            .and_then(|c| c.child("EmailAddresses"))
            .and_then(|e| e.all("Entry").find(|x| x.text.trim().to_ascii_lowercase().starts_with("smtp:")))
            .map(|e| smtp(e.text.trim()))
            .unwrap_or_default(),
    };
    let phones: Vec<(&str, &str)> = contact
        .and_then(|c| c.child("PhoneNumbers"))
        .into_iter()
        .flat_map(|p| p.all("Entry"))
        .filter(|e| !e.text.trim().is_empty())
        .map(|e| (e.attr("Key").unwrap_or(""), e.text.trim()))
        .collect();
    let phone = ["BusinessPhone", "MobilePhone"]
        .iter()
        .find_map(|k| phones.iter().find(|(key, _)| key == k))
        .or(phones.first())
        .map(|(_, v)| (*v).to_owned())
        .unwrap_or_default();
    let name = match c(&["DisplayName"]) {
        n if !n.is_empty() => n,
        _ => mailbox.str_at(&["Name"]),
    };
    Some(Person {
        name,
        email,
        title: c(&["JobTitle"]),
        department: c(&["Department"]),
        phone,
        company: c(&["CompanyName"]),
        office: c(&["OfficeLocation"]),
        group: mailbox.text_at(&["MailboxType"]) == Some("PublicDL"),
    })
}

/// Найденные: «не найдено» — пусто; несколько совпадений (`Warning`) — все.
pub(crate) fn parse(resp: &El, max: usize) -> Result<Vec<Person>> {
    let Some(msg) = soap::messages(resp).into_iter().next() else { return Ok(Vec::new()) };
    if soap::code(msg) == "ErrorNameResolutionNoResults" {
        return Ok(Vec::new());
    }
    soap::check(msg)?;
    Ok(msg.child("ResolutionSet").into_iter().flat_map(|s| s.all("Resolution")).filter_map(person).take(max).collect())
}

impl Client {
    /// Люди и списки справочника по строке (начало имени, фамилии, адреса), не больше `max`.
    pub async fn resolve_names(&mut self, query: &str, max: usize) -> Result<Vec<Person>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.call("ResolveNames", &request(query)).await?;
        let people = parse(&resp, max)?;
        tracing::debug!(found = people.len(), "ews resolve names");
        Ok(people)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::parse as xml;

    fn resp(b: &[u8]) -> El {
        soap::response(xml(b).unwrap()).unwrap()
    }

    #[test]
    fn people_and_groups() {
        let p = parse(&resp(include_bytes!("../tests/fixtures/resolve_names.xml")), 10).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(
            p[0],
            Person {
                name: "Петрова Анна Сергеевна".into(),
                email: "anna@example.test".into(),
                title: "Ведущий инженер".into(),
                department: "Отдел разработки".into(),
                phone: "+7 495 000-00-01".into(),
                company: "Пример".into(),
                office: "Москва, 5 этаж".into(),
                group: false,
            }
        );
        assert_eq!((p[1].email.as_str(), p[1].phone.as_str()), ("anton@example.test", "+7 900 000-00-02"));
        assert!(p[2].group && p[2].email == "dev-team@example.test");
        assert_eq!(parse(&resp(include_bytes!("../tests/fixtures/resolve_names.xml")), 1).unwrap().len(), 1);
    }

    #[test]
    fn nothing_found() {
        assert!(parse(&resp(include_bytes!("../tests/fixtures/resolve_names_none.xml")), 10).unwrap().is_empty());
    }

    #[test]
    fn query_is_escaped() {
        assert!(request("a<b").contains("<m:UnresolvedEntry>a&lt;b</m:UnresolvedEntry>"));
    }
}
