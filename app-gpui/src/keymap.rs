//! Свои клавиши (S5-09, «Настройки → Клавиатура»): поверх реестра команд (`commands.rs`) — назначения
//! пользователя из `postman_core::prefs::Prefs::keys`. Здесь — итоговые клавиши команды, где они действуют,
//! конфликты (одно сочетание у двух команд там, где обе действуют) и запись сочетания для таблицы.
//! Привязка клавиш с учётом своих — `commands::bind`.

use std::collections::BTreeMap;

use pets_commands::{Command, parse_keys};

use crate::commands::COMMANDS;

/// Свои клавиши: id команды → сочетания (пустой список — без клавиши).
pub(crate) type Overrides = BTreeMap<String, Vec<String>>;

/// `ctrl-shift-r` → `Ctrl+Shift+R`, `pagedown` → `PgDn`.
pub(crate) fn human(keys: &str) -> String {
    let stroke = |s: &str| {
        let mut parts: Vec<&str> = s.split('-').collect();
        // `ctrl--` — клавиша «минус».
        if parts.len() > 1 && parts.last() == Some(&"") {
            parts.pop();
            if let Some(last) = parts.last_mut() {
                *last = "-";
            }
        }
        parts
            .iter()
            .map(|k| match *k {
                "ctrl" => "Ctrl".to_string(),
                "shift" => "Shift".to_string(),
                "alt" => "Alt".to_string(),
                "win" | "super" | "cmd" => "Win".to_string(),
                "escape" => "Esc".to_string(),
                "enter" => "Enter".to_string(),
                "tab" => "Tab".to_string(),
                "space" => "Space".to_string(),
                "backspace" => "Backspace".to_string(),
                "delete" => "Delete".to_string(),
                "insert" => "Insert".to_string(),
                "home" => "Home".to_string(),
                "end" => "End".to_string(),
                "pagedown" => "PgDn".to_string(),
                "pageup" => "PgUp".to_string(),
                "up" => "↑".to_string(),
                "down" => "↓".to_string(),
                "left" => "←".to_string(),
                "right" => "→".to_string(),
                k => k.to_uppercase(),
            })
            .collect::<Vec<_>>()
            .join("+")
    };
    keys.split(' ').filter(|s| !s.is_empty()).map(stroke).collect::<Vec<_>>().join(" ")
}

/// Где действуют клавиши команды — по её контексту.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// Окно письма.
    Composer,
    /// Вкладка встречи.
    Meeting,
    /// Раздел «Почта» главного окна.
    Mail,
    /// Раздел «Календарь».
    Calendar,
    /// Раздел «Контакты» (S5-11).
    Contacts,
    /// Палитра команд (S5-10).
    Palette,
    /// Всё главное окно.
    Main,
}

impl Place {
    pub(crate) fn of(context: Option<&str>) -> Place {
        // «… && !Composer» — клавиши почты вне письма (письмо вкладкой и ответ в области чтения, S5-06б): это почта.
        let context = context.map(|c| c.replace("!Composer", "").replace("!ContextMenu", ""));
        match context.as_deref() {
            Some(c) if c.contains("Palette") => Place::Palette,
            Some(c) if c.contains("Composer") => Place::Composer,
            Some(c) if c.contains("Meeting") => Place::Meeting,
            Some(c) if c.contains("CalendarView") => Place::Calendar,
            Some(c) if c.contains("ContactsView") => Place::Contacts,
            Some(c) if c.contains("MailView") => Place::Mail,
            _ => Place::Main,
        }
    }

    /// Ключ текста «Где работает» (`settings-keys-where-…`).
    pub(crate) fn key(self) -> &'static str {
        match self {
            Place::Composer => "settings-keys-where-composer",
            Place::Meeting => "settings-keys-where-meeting",
            Place::Mail => "settings-keys-where-mail",
            Place::Calendar => "settings-keys-where-calendar",
            Place::Contacts => "settings-keys-where-contacts",
            Place::Palette => "settings-keys-where-palette",
            Place::Main => "settings-keys-where-main",
        }
    }

    /// Клавиши двух команд могут сработать в одном месте: одно окно, и разделы совпадают или один из них — всё окно.
    pub(crate) fn overlaps(self, other: Place) -> bool {
        use Place::*;
        match (self, other) {
            (Composer, Composer) | (Meeting, Meeting) | (Palette, Palette) => true,
            (Composer | Meeting | Palette, _) | (_, Composer | Meeting | Palette) => false,
            (Main, _) | (_, Main) => true,
            (a, b) => a == b,
        }
    }
}

/// Клавиши команды можно поменять. Клавиши 1–9 — категории таблицы (S5-02): по одной на категорию, меняются
/// в «Категориях».
pub(crate) fn editable(c: &Command) -> bool {
    c.id != "mail.categorize"
}

/// Итоговые клавиши команды: свои или по умолчанию.
pub(crate) fn effective(c: &Command, overrides: &Overrides) -> Vec<String> {
    match overrides.get(c.id).filter(|_| editable(c)) {
        Some(keys) => keys.clone(),
        None => c.keys.iter().map(|k| k.to_string()).collect(),
    }
}

/// Каноническая запись сочетания (модификаторы по алфавиту) — для сравнения.
fn canon(keys: &str) -> String {
    parse_keys(keys).unwrap_or_else(|_| keys.to_owned())
}

/// Одно сочетание у двух команд, которые действуют в одном месте.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Conflict {
    /// Сочетание в записи gpui.
    pub(crate) keys: String,
    pub(crate) first: &'static str,
    pub(crate) second: &'static str,
}

/// Все конфликты таблицы со своими клавишами.
pub(crate) fn conflicts(commands: &[Command], overrides: &Overrides) -> Vec<Conflict> {
    let mut bound: Vec<(String, Place, &'static str)> = Vec::new();
    let mut out = Vec::new();
    for c in commands {
        let place = Place::of(c.context);
        for keys in effective(c, overrides) {
            let k = canon(&keys);
            for &(_, _, first) in bound.iter().filter(|(bk, p, id)| *bk == k && p.overlaps(place) && *id != c.id) {
                out.push(Conflict { keys: keys.clone(), first, second: c.id });
            }
            bound.push((k, place, c.id));
        }
    }
    out
}

/// Команда, у которой это сочетание уже есть там, где действует `id` (кроме самой `id`).
pub(crate) fn taken_by(commands: &[Command], overrides: &Overrides, id: &str, keys: &str) -> Option<&'static str> {
    let place = Place::of(commands.iter().find(|c| c.id == id)?.context);
    let k = canon(keys);
    commands
        .iter()
        .filter(|c| c.id != id && Place::of(c.context).overlaps(place))
        .find(|c| effective(c, overrides).iter().any(|e| canon(e) == k))
        .map(|c| c.id)
}

/// Назначить команде `id` сочетание `keys` (вместо прежних). `replace` — снять его с команд, у которых оно было
/// в том же месте. Своё назначение, совпавшее с умолчанием, не хранится. Ошибка — сочетание не разобрано.
pub(crate) fn assign(commands: &[Command], overrides: &mut Overrides, id: &str, keys: &str, replace: bool) -> Result<(), String> {
    let canonical = parse_keys(keys)?;
    let Some(c) = commands.iter().find(|c| c.id == id).filter(|c| editable(c)) else {
        return Err(format!("{id}: keys are fixed"));
    };
    if replace {
        while let Some(other) = taken_by(commands, overrides, id, keys) {
            let Some(o) = commands.iter().find(|c| c.id == other) else { break };
            let rest: Vec<String> = effective(o, overrides).into_iter().filter(|e| canon(e) != canonical).collect();
            set(overrides, o, rest);
        }
    }
    set(overrides, c, vec![keys.to_owned()]);
    Ok(())
}

/// Ревизия клавиш по умолчанию (`Prefs::keys_rev`): растёт, когда реестр снимает или переносит клавиши.
/// 1 — S5-16, правило одного модификатора (`docs/DESIGN.md` «Хоткеи»).
pub(crate) const KEYS_REV: u32 = 1;

/// Клавиши по умолчанию, которые S5-16 сняла с команд: (команда, сочетание).
const REMOVED_1: &[(&str, &str)] = &[
    ("mail.next", "j"),
    ("mail.prev", "k"),
    ("mail.toggle_flag", "insert"),
    ("mail.due", "ctrl-shift-g"),
    ("calendar.prev_week", "alt-up"),
    ("calendar.next_week", "alt-down"),
    ("calendar.today", "alt-home"),
    ("compose.send", "alt-s"),
    ("meeting.send", "alt-s"),
    ("view.filters", "ctrl-shift-f"),
    ("invite.accept", "a"),
    ("invite.tentative", "t"),
    ("invite.decline", "n"),
];

/// Клавиши по умолчанию, которые S5-16 дала командам.
const ADDED_1: &[(&str, &str)] = &[
    ("mail.due", "insert"),
    ("calendar.prev_week", "pageup"),
    ("calendar.next_week", "pagedown"),
    ("calendar.today", "home"),
    ("tab.new", "ctrl-t"),
];

/// Свои клавиши ревизии `rev` — к [`KEYS_REV`]. Из прежних умолчаний в своих списках (остаются после «Заменить»)
/// снятые клавиши уходят; своё, совпавшее с новым умолчанием, и назначения несуществующих команд не хранятся;
/// новое умолчание, которое пользователь уже отдал другой команде там же, остаётся у его команды (у новой — без
/// этой клавиши). Свои сочетания, выбранные пользователем, не трогаются — даже вне правила.
pub(crate) fn migrate(commands: &[Command], overrides: &mut Overrides, rev: u32) {
    if rev >= KEYS_REV {
        return;
    }
    overrides.retain(|id, _| commands.iter().any(|c| c.id == id && editable(c)));
    let ids: Vec<String> = overrides.keys().cloned().collect();
    for id in ids {
        let Some(c) = commands.iter().find(|c| c.id == id) else { continue };
        let Some(keys) = overrides.get(&id) else { continue };
        let kept: Vec<String> =
            keys.iter().filter(|k| !REMOVED_1.iter().any(|(cid, r)| *cid == id && canon(k) == canon(r))).cloned().collect();
        set(overrides, c, kept);
    }
    for &(id, key) in ADDED_1 {
        let Some(c) = commands.iter().find(|c| c.id == id) else { continue };
        if overrides.contains_key(id) || taken_by(commands, overrides, id, key).is_none() {
            continue;
        }
        let rest: Vec<String> = c.keys.iter().filter(|k| canon(k) != canon(key)).map(|k| k.to_string()).collect();
        set(overrides, c, rest);
    }
}

/// Сбросить клавиши команды к умолчанию.
pub(crate) fn reset(overrides: &mut Overrides, id: &str) {
    overrides.remove(id);
}

fn set(overrides: &mut Overrides, c: &Command, keys: Vec<String>) {
    let default: Vec<String> = c.keys.iter().map(|k| k.to_string()).collect();
    if keys == default {
        overrides.remove(c.id);
    } else {
        overrides.insert(c.id.to_owned(), keys);
    }
}

/// Сочетание нажатия для таблицы: `None` — одни модификаторы (ждём клавишу).
pub(crate) fn recorded(unparsed: &str) -> Option<String> {
    let unparsed = unparsed.to_lowercase();
    let unparsed = unparsed.as_str();
    let last = unparsed.rsplit('-').next().unwrap_or_default();
    let minus = unparsed == "-" || unparsed.ends_with("--");
    if !minus && ["shift", "control", "alt", "platform", "function", "ctrl", "win", "cmd", "super", "fn", ""].contains(&last) {
        return None;
    }
    parse_keys(unparsed).ok()
}

/// Команды для таблицы «Клавиатура» по поиску: название, id или клавиша (`ctrl+r`, `Ctrl+R`).
pub(crate) fn search(query: &str, name: impl Fn(&Command) -> String, overrides: &Overrides) -> Vec<&'static Command> {
    let q = query.trim().to_lowercase();
    COMMANDS
        .iter()
        .filter(|c| {
            if q.is_empty() {
                return true;
            }
            let keys: Vec<String> = effective(c, overrides).iter().map(|k| human(k).to_lowercase()).collect();
            name(c).to_lowercase().contains(&q) || c.id.contains(&q) || keys.iter().any(|k| k.contains(&q))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn cmd(id: &'static str, keys: &'static [&'static str], context: Option<&'static str>) -> Command {
        Command { id, keys, context }
    }

    const TABLE: &[Command] = &[
        cmd("mail.reply", &["ctrl-r"], Some("MailView")),
        cmd("mail.next", &["j"], Some("MailView && !Input")),
        cmd("calendar.today", &["t"], Some("CalendarView")),
        cmd("search.clear", &["escape"], None),
        cmd("compose.close", &["escape"], Some("Composer || (Composer > Input)")),
        cmd("tab.close", &["ctrl-w"], Some("Mailer")),
    ];

    /// Реестр Postman без своих клавиш — без конфликтов (Esc в окне письма и в главном — разные окна).
    #[test]
    fn registry_has_no_conflicts() {
        assert_eq!(conflicts(COMMANDS, &Overrides::new()), vec![]);
        assert_eq!(conflicts(TABLE, &Overrides::new()), vec![]);
    }

    /// Своё сочетание, занятое другой командой там же, — конфликт; в другом разделе или окне — нет.
    #[test]
    fn override_conflicts_where_both_work() {
        let mut o = Overrides::new();
        o.insert("mail.next".into(), vec!["ctrl-r".into()]);
        let c = conflicts(TABLE, &o);
        assert_eq!(c, vec![Conflict { keys: "ctrl-r".into(), first: "mail.reply", second: "mail.next" }]);
        // Календарь и почта — разные разделы.
        let mut o = Overrides::new();
        o.insert("calendar.today".into(), vec!["j".into()]);
        assert_eq!(conflicts(TABLE, &o), vec![]);
        // «Всё окно» пересекается с любым разделом главного окна, но не с окном письма.
        let mut o = Overrides::new();
        o.insert("tab.close".into(), vec!["shift-ctrl-r".into()]);
        o.insert("mail.reply".into(), vec!["ctrl-shift-r".into()]);
        assert_eq!(conflicts(TABLE, &o).len(), 1, "модификаторы в любом порядке — одно сочетание");
        assert_eq!(taken_by(TABLE, &Overrides::new(), "compose.close", "escape"), None);
        assert_eq!(taken_by(TABLE, &Overrides::new(), "tab.close", "ctrl-r"), Some("mail.reply"));
    }

    /// «Заменить» снимает сочетание с прежней команды; без замены — оба (конфликт виден); сброс — умолчание.
    #[test]
    fn assign_replace_and_reset() {
        let mut o = Overrides::new();
        assign(TABLE, &mut o, "tab.close", "ctrl-r", false).unwrap();
        assert_eq!(conflicts(TABLE, &o).len(), 1);
        let mut o = Overrides::new();
        assign(TABLE, &mut o, "tab.close", "ctrl-r", true).unwrap();
        assert_eq!(o.get("mail.reply"), Some(&vec![]), "у «Ответить» клавиши больше нет");
        assert_eq!(o.get("tab.close"), Some(&vec!["ctrl-r".to_string()]));
        assert_eq!(conflicts(TABLE, &o), vec![]);
        reset(&mut o, "tab.close");
        assert_eq!(effective(&TABLE[5], &o), ["ctrl-w"]);
        // Назначили то же, что по умолчанию, — своего назначения нет.
        let mut o = Overrides::new();
        assign(TABLE, &mut o, "mail.reply", "ctrl-r", false).unwrap();
        assert!(o.is_empty());
        assert!(assign(TABLE, &mut o, "mail.reply", "ctrl-шум", false).is_err());
    }

    /// S5-16: свои клавиши прежней ревизии — без снятых умолчаний; совпало с новым умолчанием — своего нет; новое
    /// умолчание, занятое своим, — у своего; своё вне правила остаётся; ревизия уже новая — ничего.
    #[test]
    fn overrides_migrate_from_before_s5_16() {
        let mut o = Overrides::new();
        // «Заменить» отдал ↓ другой команде: у «Следующее» осталось прежнее J.
        o.insert("mail.next".into(), vec!["j".into()]);
        o.insert("mail.select_down".into(), vec!["down".into()]);
        // «Заменить» отдал Insert «Напомнить»: у флага — пусто (теперь это и есть умолчание).
        o.insert("mail.remind".into(), vec!["insert".into()]);
        o.insert("mail.toggle_flag".into(), vec![]);
        // Свой Alt+S к Ctrl+Enter у письма — только прежнее умолчание Alt+S уходит.
        o.insert("compose.send".into(), vec!["ctrl-enter".into(), "alt-s".into(), "ctrl-s".into()]);
        // Своё вне правила — выбор пользователя.
        o.insert("mail.reply".into(), vec!["alt-r".into()]);
        // Ctrl+T у пересылки — новая «Новая вкладка» без клавиши.
        o.insert("mail.forward".into(), vec!["ctrl-t".into()]);
        o.insert("no.such_command".into(), vec!["f9".into()]);
        migrate(COMMANDS, &mut o, 0);
        assert_eq!(o.get("mail.next"), Some(&vec![]), "J ушла, ↓ — у другой команды");
        assert_eq!(o.get("mail.toggle_flag"), None, "пусто — уже умолчание");
        assert_eq!(o.get("mail.remind"), Some(&vec!["insert".to_string()]));
        assert_eq!(o.get("mail.due"), Some(&vec![]), "Insert — у «Напомнить», как выбрал пользователь");
        assert_eq!(o.get("compose.send"), Some(&vec!["ctrl-enter".to_string(), "ctrl-s".to_string()]));
        assert_eq!(o.get("mail.reply"), Some(&vec!["alt-r".to_string()]));
        assert_eq!(o.get("tab.new"), Some(&vec![]));
        assert_eq!(o.get("no.such_command"), None);
        assert_eq!(conflicts(COMMANDS, &o), vec![], "перевод не добавил конфликтов");
        // Только Alt+S было своим — после перевода своего нет.
        let mut o = Overrides::new();
        o.insert("meeting.send".into(), vec!["ctrl-enter".into(), "alt-s".into()]);
        migrate(COMMANDS, &mut o, 0);
        assert!(o.is_empty());
        // Ревизия текущая: своё J — выбор пользователя после S5-16.
        let mut o = Overrides::new();
        o.insert("mail.next".into(), vec!["j".into()]);
        migrate(COMMANDS, &mut o, KEYS_REV);
        assert_eq!(o.get("mail.next"), Some(&vec!["j".to_string()]));
    }

    #[test]
    fn categories_keys_are_fixed() {
        let c = pets_commands::find(COMMANDS, "mail.categorize").unwrap();
        let mut o = Overrides::new();
        o.insert(c.id.into(), vec![]);
        assert_eq!(effective(c, &o).len(), 9);
        assert!(assign(COMMANDS, &mut o, c.id, "ctrl-1", false).is_err());
    }

    #[test]
    fn keystrokes_are_recorded_without_bare_modifiers() {
        assert_eq!(recorded("ctrl-shift-r").as_deref(), Some("ctrl-shift-r"));
        assert_eq!(recorded("shift-ctrl-k").as_deref(), Some("ctrl-shift-k"));
        assert_eq!(recorded("ctrl-control"), None);
        assert_eq!(recorded("shift"), None);
        assert_eq!(recorded("f5").as_deref(), Some("f5"));
    }

    #[test]
    fn keys_read_like_windows() {
        assert_eq!(human("ctrl-shift-r"), "Ctrl+Shift+R");
        assert_eq!(human("alt-down"), "Alt+↓");
        assert_eq!(human("j"), "J");
        assert_eq!(human("ctrl--"), "Ctrl+-");
        assert_eq!(human("ctrl-,"), "Ctrl+,");
    }
}
