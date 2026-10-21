//! Не Windows: хранилищ и СКЗИ нет — якорей нет, цепочка в ОС не подтверждается.

use postman_smime::Trust;

use super::Revocation;

pub(super) fn load_trust(_corporate_extra: &[String]) -> Trust {
    Trust::default()
}

pub(super) fn confirm(_chain: &[Vec<u8>], _exclusive_root: Option<&[u8]>) -> Revocation {
    Revocation::NotChecked
}

pub(super) fn gost_provider() -> Option<String> {
    None
}
