//! Список писем: окно строк в фоне, выделение, флаги (S1-03, S1-06); полоса прокрутки и позиция в каждой
//! папке, строка «Удалить навсегда?» (S3-02).

use std::collections::HashMap;
use std::ops::Range;

use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::*;
use gpui_kit::prelude::*;
use pets_i18n::tr;
use postman_cache::{Cache, EmailRow};

use crate::*;

pub(crate) const ROW_HEIGHT: f32 = 44.0;
pub(crate) const WINDOW_MARGIN: usize = 100;

/// Запись флагов в хранилище по очереди в отдельном потоке: поток интерфейса не ждёт диск
/// (каждый коммит redb — fsync, на листании с открытием писем это до 30 мс на кадр).
pub(crate) struct FlagWriter {
    pub(crate) tx: std::sync::mpsc::Sender<(String, bool, bool)>,
    pub(crate) cache: Cache,
}

impl FlagWriter {
    pub(crate) fn new(cache: Cache) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<(String, bool, bool)>();
        let writer = cache.clone();
        // Не запустился поток — получатель закрыт, `set` пишет сам (медленнее, но без потерь).
        let _ = std::thread::Builder::new().name("cache-flags".into()).spawn(move || {
            for (id, seen, flagged) in rx {
                let _ = writer.set_flags(&id, seen, flagged);
            }
        });
        Self { tx, cache }
    }

    pub(crate) fn set(&self, id: &str, seen: bool, flagged: bool) {
        if self.tx.send((id.to_owned(), seen, flagged)).is_err() {
            let _ = self.cache.set_flags(id, seen, flagged);
        }
    }
}

// --- список писем ---

/// Список писем — отдельное view: при листании gpui раскладывает и рисует только его,
/// остальные части экрана берутся из прошлого кадра (S1-06).
pub(crate) struct MailList {
    pub(crate) cache: Cache,
    pub(crate) flags: FlagWriter,
    pub(crate) mailbox: Option<String>,
    /// id результатов поиска вместо папки.
    pub(crate) search_ids: Option<Vec<String>>,
    pub(crate) count: usize,
    pub(crate) rows: (usize, Vec<EmailRow>),
    /// Окно строк, которое читается в фоне, и номер запроса: ответы на устаревшие запросы отбрасываются.
    pub(crate) page_pending: Option<Range<usize>>,
    pub(crate) page_gen: u64,
    /// Письмо выбрано, а его строка ещё не прочитана — открыть, когда страница придёт.
    pub(crate) pending_open: Option<usize>,
    pub(crate) visible: Range<usize>,
    pub(crate) selected: Option<usize>,
    pub(crate) scroll: UniformListScrollHandle,
    /// Прокрутка каждой папки: вернулись в папку — список там же, где был.
    pub(crate) positions: Positions,
    /// Строка «Удалить навсегда? Да / Нет» над списком: письмо из «Удалённых» и его номер.
    pub(crate) confirm: Option<(usize, EmailRow)>,
    /// Догрузка с сервера при прокрутке (живой ящик, S4-03).
    pub(crate) more: More,
}

/// Догрузка папки: в кэше первые страницы, остальное — с сервера, когда прокрутка подходит к концу.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct More {
    /// Писем в папке на сервере (`Mailbox.totalEmails`); `None` — демо или поиск: догружать нечего.
    pub(crate) total: Option<usize>,
    /// Страница запрошена, ответа (`LoadedMore`) ещё нет.
    pub(crate) asked: bool,
    /// Сервер вернул пустую страницу (или ошибку): не просить, пока не изменится число писем в папке.
    pub(crate) exhausted: bool,
}

impl More {
    /// Просить следующую страницу: до конца загруженного меньше запаса окна, а на сервере писем больше.
    pub(crate) fn wanted(&self, visible_end: usize, count: usize) -> bool {
        !self.asked
            && !self.exhausted
            && self.total.is_some_and(|t| count < t)
            && visible_end + WINDOW_MARGIN >= count
    }

    /// Число писем на сервере (папки перечитаны); изменилось — снова можно просить.
    pub(crate) fn set_total(&mut self, total: Option<usize>) {
        if self.total != total {
            self.exhausted = false;
        }
        self.total = total;
    }

    /// Ответ на догрузку: `added` писем пришло.
    pub(crate) fn loaded(&mut self, added: usize) {
        self.asked = false;
        self.exhausted = added == 0;
    }
}

/// Список подошёл к концу загруженного: догрузить папку с сервера (`Command::LoadMore`).
pub(crate) struct NeedMore(pub(crate) String);

impl EventEmitter<NeedMore> for MailList {}

/// Позиция прокрутки (смещение по вертикали, px) каждой папки; результаты поиска — всегда сверху.
#[derive(Default)]
pub(crate) struct Positions(HashMap<String, f32>);

impl Positions {
    /// Уходим из вида `from` (папка; `None` — поиск) с прокруткой `y`; куда прокрутить вид `to`.
    pub(crate) fn switch(&mut self, from: Option<&str>, y: f32, to: Option<&str>) -> f32 {
        if let Some(mb) = from {
            self.0.insert(mb.to_owned(), y);
        }
        to.and_then(|mb| self.0.get(mb)).copied().unwrap_or(0.)
    }
}

/// Видимые строки при прокрутке `y` (смещение содержимого вверх — отрицательное) и прежнем их числе.
pub(crate) fn rows_at(y: f32, len: usize) -> Range<usize> {
    let start = (-y / ROW_HEIGHT).max(0.) as usize;
    start..start + len
}

/// «Да» в строке «Удалить навсегда?»: письмо и его номер в списке.
pub(crate) struct Purge {
    pub(crate) index: usize,
    pub(crate) row: EmailRow,
}

impl EventEmitter<Purge> for MailList {}

/// Письмо открыто (строка уже помечена прочитанной).
pub(crate) struct OpenRow(pub(crate) EmailRow);

impl EventEmitter<OpenRow> for MailList {}

/// Флаг письма изменён пользователем (в кэше уже записан): живой ящик отдаёт его серверу (`Keyword`).
pub(crate) struct Flagged {
    pub(crate) id: String,
    pub(crate) keyword: &'static str,
    pub(crate) on: bool,
}

impl EventEmitter<Flagged> for MailList {}

/// Ключевые слова JMAP, которые меняются при переходе флагов строки `old` в (`seen`, `flagged`).
pub(crate) fn keyword_changes(old: &EmailRow, seen: bool, flagged: bool) -> Vec<(&'static str, bool)> {
    let mut out = Vec::new();
    if old.seen != seen {
        out.push(("$seen", seen));
    }
    if old.flagged != flagged {
        out.push(("$flagged", flagged));
    }
    out
}

/// Выделение после перечитывания: то же письмо по id в новом окне строк; не нашлось — без выделения.
pub(crate) fn reselect(id: Option<&str>, offset: usize, rows: &[EmailRow], count: usize) -> Option<usize> {
    let pos = rows.iter().position(|r| Some(r.id.as_str()) == id)?;
    Some(offset + pos).filter(|&i| i < count)
}

impl MailList {
    pub(crate) fn new(cache: Cache) -> Self {
        Self {
            flags: FlagWriter::new(cache.clone()),
            cache,
            mailbox: None,
            search_ids: None,
            count: 0,
            rows: (0, Vec::new()),
            page_pending: None,
            page_gen: 0,
            pending_open: None,
            visible: 0..30,
            selected: None,
            scroll: UniformListScrollHandle::new(),
            positions: Positions::default(),
            confirm: None,
            more: More::default(),
        }
    }

    pub(crate) fn show_mailbox(&mut self, id: String, cx: &mut Context<Self>) {
        // Другая папка — своя догрузка; число писем на сервере ставит экран (`Mailer::sync_total`).
        if self.mailbox.as_deref() != Some(id.as_str()) {
            self.more = More::default();
        }
        self.switch_view(Some(id), None);
        self.selected = None;
        self.reload();
        cx.notify();
    }

    pub(crate) fn show_search(&mut self, ids: Option<Vec<String>>, cx: &mut Context<Self>) {
        let mailbox = self.mailbox.clone();
        self.switch_view(mailbox, ids);
        self.selected = None;
        self.reload();
        cx.notify();
    }

    /// Первые результаты поиска живого ящика (дальше они дополняются через `refresh`).
    pub(crate) fn begin_search(&mut self, ids: Vec<String>) {
        let mailbox = self.mailbox.clone();
        self.switch_view(mailbox, Some(ids));
        self.selected = None;
    }

    /// Другая папка или поиск: запомнить прокрутку прежнего вида, поставить прокрутку нового.
    fn switch_view(&mut self, mailbox: Option<String>, search: Option<Vec<String>>) {
        let key = |mailbox: &Option<String>, search: &Option<Vec<String>>| match search {
            Some(_) => None,
            None => mailbox.clone(),
        };
        let from = key(&self.mailbox, &self.search_ids);
        let to = key(&mailbox, &search);
        let y = self.positions.switch(from.as_deref(), self.offset_y(), to.as_deref());
        self.mailbox = mailbox;
        self.search_ids = search;
        self.confirm = None;
        self.set_offset_y(y);
        self.visible = rows_at(y, self.visible.len());
    }

    fn offset_y(&self) -> f32 {
        f32::from(self.scroll.0.borrow().base_handle.offset().y)
    }

    fn set_offset_y(&self, y: f32) {
        let mut s = self.scroll.0.borrow_mut();
        s.deferred_scroll_to_item = None;
        s.base_handle.set_offset(point(px(0.), px(y)));
    }

    pub(crate) fn selected_row(&self) -> Option<EmailRow> {
        self.selected.and_then(|i| self.row(i)).cloned()
    }

    /// «Удалить» в «Удалённых»: строка подтверждения над списком.
    pub(crate) fn ask_purge(&mut self, index: usize, row: EmailRow, cx: &mut Context<Self>) {
        self.confirm = Some((index, row));
        cx.notify();
    }

    /// Ответ на «Удалить навсегда?»; `false` — «Нет» и Esc.
    pub(crate) fn answer_purge(&mut self, yes: bool, cx: &mut Context<Self>) {
        if let Some((index, row)) = self.confirm.take()
            && yes
        {
            cx.emit(Purge { index, row });
        }
        cx.notify();
    }

    /// Письмо ушло из списка (в «Удалённые» или насовсем): перечитать и выделить следующее — оно теперь на
    /// месте удалённого.
    pub(crate) fn removed(&mut self, id: &str, index: usize, cx: &mut Context<Self>) {
        if let Some(ids) = &mut self.search_ids {
            ids.retain(|x| x != id);
        }
        self.confirm = None;
        self.reread(Some(index), cx);
    }

    pub(crate) fn reload(&mut self) {
        self.count = match (&self.search_ids, &self.mailbox) {
            (Some(ids), _) => ids.len(),
            (None, Some(mb)) => self.cache.count(mb).unwrap_or(0),
            _ => 0,
        };
        // Первое окно — сразу: первый кадр папки рисуется со строками, а не с заглушками.
        self.page_gen += 1;
        self.page_pending = None;
        self.pending_open = None;
        let (offset, limit) = page_window(&self.visible);
        self.rows = (offset, self.page_source().read(&self.cache, offset, limit));
    }

    /// Данные изменились (синхронизация): число писем и окно строк — в фоне, выделение остаётся на том же письме.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reread(None, cx);
    }

    /// Перечитать в фоне; `select` — затем выделить и открыть письмо с этим номером (иначе — то же письмо).
    fn reread(&mut self, select: Option<usize>, cx: &mut Context<Self>) {
        let selected = self.selected.and_then(|i| self.row(i)).map(|r| r.id.clone());
        let (offset, limit) = page_window(&self.visible);
        self.page_gen += 1;
        let generation = self.page_gen;
        self.page_pending = Some(offset..offset + limit);
        let (source, cache) = (self.page_source(), self.cache.clone());
        let task = cx.background_spawn(async move { (source.count(&cache), source.read(&cache, offset, limit)) });
        cx.spawn(async move |this, cx| {
            let (count, rows) = task.await;
            let _ = this.update(cx, |m, cx| {
                if m.page_gen != generation {
                    return;
                }
                m.page_pending = None;
                m.count = count;
                m.selected = reselect(selected.as_deref(), offset, &rows, count);
                m.rows = (offset, rows);
                if let Some(i) = select {
                    m.selected = None;
                    if count > 0 {
                        m.select(i.min(count - 1), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Окно строк вокруг видимого диапазона. Читается в фоне — поток интерфейса не ждёт хранилище;
    /// следующее окно запрашивается заранее, когда до края загруженного меньше половины запаса.
    pub(crate) fn ensure(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let (off, rows) = (self.rows.0, self.rows.1.len());
        let end = off + rows;
        let loaded = rows > 0 && range.start >= off && range.end <= end;
        let near_edge = (off > 0 && range.start < off + WINDOW_MARGIN / 2)
            || (end < self.count && range.end + WINDOW_MARGIN / 2 > end);
        if loaded && !near_edge {
            return;
        }
        if self.page_pending.as_ref().is_some_and(|p| range.start >= p.start && range.end <= p.end) {
            return;
        }
        let (offset, limit) = page_window(&range);
        self.page_gen += 1;
        let generation = self.page_gen;
        self.page_pending = Some(offset..offset + limit);
        let (source, cache) = (self.page_source(), self.cache.clone());
        let task = cx.background_spawn(async move { source.read(&cache, offset, limit) });
        cx.spawn(async move |this, cx| {
            let rows = task.await;
            let _ = this.update(cx, |m, cx| {
                if m.page_gen != generation {
                    return;
                }
                m.page_pending = None;
                m.rows = (offset, rows);
                if let Some(row) = m.pending_open.take().and_then(|i| m.row(i).cloned()) {
                    m.open_row(row, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Видно конец загруженного — попросить следующую страницу у сервера (один запрос до ответа).
    fn ask_more(&mut self, visible_end: usize, cx: &mut Context<Self>) {
        let Some(mb) = self.mailbox.clone().filter(|_| self.search_ids.is_none()) else { return };
        if self.more.wanted(visible_end, self.count) {
            self.more.asked = true;
            cx.emit(NeedMore(mb));
        }
    }

    /// Откуда читать строки списка: папка или результаты поиска.
    pub(crate) fn page_source(&self) -> PageSource {
        match (&self.search_ids, &self.mailbox) {
            (Some(ids), _) => PageSource::Ids(ids.clone()),
            (None, Some(mb)) => PageSource::Mailbox(mb.clone()),
            _ => PageSource::Empty,
        }
    }

    pub(crate) fn row(&self, i: usize) -> Option<&EmailRow> {
        i.checked_sub(self.rows.0).and_then(|j| self.rows.1.get(j))
    }

    pub(crate) fn select(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.count == 0 {
            return;
        }
        let i = i.min(self.count - 1);
        // Другое письмо — вопрос об удалении прежнего снят.
        if self.confirm.as_ref().is_some_and(|(c, _)| *c != i) {
            self.confirm = None;
        }
        self.selected = Some(i);
        self.scroll.scroll_to_item(i, ScrollStrategy::Center);
        self.ensure(i..i + 1, cx);
        cx.notify();
        let Some(row) = self.row(i).cloned() else {
            self.pending_open = Some(i);
            return;
        };
        self.pending_open = None;
        self.open_row(row, cx);
    }

    /// При первом открытии письмо помечается прочитанным, затем уходит в область чтения.
    pub(crate) fn open_row(&mut self, mut row: EmailRow, cx: &mut Context<Self>) {
        if !row.seen {
            row.seen = true;
            self.flags.set(&row.id, true, row.flagged);
            self.patch_row(&row);
            cx.emit(Flagged { id: row.id.clone(), keyword: "$seen", on: true });
        }
        cx.emit(OpenRow(row));
    }

    pub(crate) fn patch_row(&mut self, row: &EmailRow) {
        for r in self.rows.1.iter_mut().filter(|r| r.id == row.id) {
            *r = row.clone();
        }
    }

    pub(crate) fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let cur = self.selected.map_or(0, |i| i as isize);
        let next = if self.selected.is_none() { 0 } else { (cur + delta).max(0) as usize };
        self.select(next, cx);
    }

    pub(crate) fn set_flags(&mut self, seen: Option<bool>, toggle_flag: bool, cx: &mut Context<Self>) {
        let Some(mut row) = self.selected.and_then(|i| self.row(i)).cloned() else { return };
        let old = row.clone();
        if let Some(s) = seen {
            row.seen = s;
        }
        if toggle_flag {
            row.flagged = !row.flagged;
        }
        self.flags.set(&row.id, row.seen, row.flagged);
        self.patch_row(&row);
        for (keyword, on) in keyword_changes(&old, row.seen, row.flagged) {
            cx.emit(Flagged { id: row.id.clone(), keyword, on });
        }
        cx.notify();
    }

    pub(crate) fn page(&self) -> isize {
        self.visible.len().max(1) as isize
    }

    /// Замер: строка `i` — первая видимая.
    pub(crate) fn scroll_to_top(&mut self, i: usize, cx: &mut Context<Self>) {
        self.scroll.scroll_to_item(i, ScrollStrategy::Top);
        cx.notify();
    }
}

impl Render for MailList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let confirm = self.confirm.as_ref().map(|_| {
            let button = |id: &'static str, text: String, yes: bool| {
                div()
                    .id(id)
                    .px_3()
                    .py_0p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(yes, |d| d.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff)).hover(|d| d.bg(rgb(0xa52517))))
                    .when(!yes, |d| d.bg(rgb(0xececec)).hover(|d| d.bg(rgb(0xe0e0e0))))
                    .child(text)
                    .on_click(cx.listener(move |this, _, _, cx| this.answer_purge(yes, cx)))
            };
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .bg(rgb(0xfff4ce))
                .border_b_1()
                .border_color(rgb(0xdddddd))
                .child(div().flex_1().child(tr!("actions-purge-confirm")))
                .child(button("purge-yes", tr!("actions-purge-yes"), true))
                .child(button("purge-no", tr!("actions-purge-no"), false))
        });
        let list = div().relative().flex_1().min_h_0().child(
            uniform_list(
                "messages",
                self.count,
                cx.processor(|this, range: Range<usize>, _window, cx| {
                    // Одну строку gpui запрашивает, чтобы измерить высоту, — это не видимая область.
                    if range.len() > 1 {
                        this.visible = range.clone();
                        this.ensure(range.clone(), cx);
                        this.ask_more(range.end, cx);
                    }
                    range
                        .map(|i| match this.row(i).cloned() {
                            Some(r) => list_row(i, &r, this.selected == Some(i), cx),
                            None => empty_row(i),
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full(),
        );
        // Полоса прокрутки видна всегда и перетаскивается; кадры — только пока её двигают.
        let list = list.child(Scrollbar::vertical(&self.scroll).mode(ScrollbarMode::Always));
        div().size_full().flex().flex_col().border_r_1().border_color(rgb(0xdddddd)).children(confirm).child(list)
    }
}

/// Окно чтения вокруг диапазона: (смещение, число строк).
pub(crate) fn page_window(range: &Range<usize>) -> (usize, usize) {
    (range.start.saturating_sub(WINDOW_MARGIN), range.len() + 2 * WINDOW_MARGIN)
}

pub(crate) enum PageSource {
    Mailbox(String),
    Ids(Vec<String>),
    Empty,
}

impl PageSource {
    pub(crate) fn count(&self, cache: &Cache) -> usize {
        match self {
            PageSource::Mailbox(mb) => cache.count(mb).unwrap_or(0),
            PageSource::Ids(ids) => ids.len(),
            PageSource::Empty => 0,
        }
    }

    pub(crate) fn read(&self, cache: &Cache, offset: usize, limit: usize) -> Vec<EmailRow> {
        match self {
            PageSource::Mailbox(mb) => cache.page(mb, offset, limit).unwrap_or_default(),
            PageSource::Ids(ids) => {
                let end = (offset + limit).min(ids.len());
                cache.rows_by_ids(&ids[offset.min(end)..end]).unwrap_or_default()
            }
            PageSource::Empty => Vec::new(),
        }
    }
}

/// Строка, которая ещё читается из хранилища: та же высота, без текста.
pub(crate) fn empty_row(i: usize) -> AnyElement {
    div().id(i).w_full().h(px(ROW_HEIGHT)).border_b_1().border_color(rgb(0xe5e5e5)).into_any_element()
}

pub(crate) fn list_row(i: usize, r: &EmailRow, selected: bool, cx: &mut Context<MailList>) -> AnyElement {
    let from = if r.from_name.is_empty() { r.from_addr.clone() } else { r.from_name.clone() };
    let mut icons = String::new();
    if r.has_attachment {
        icons.push('📎');
    }
    if r.flagged {
        icons.push('⚑');
    }
    let line2 = format!("{}  —  {}", r.subject, r.preview);
    div()
        .id(i)
        .w_full()
        .relative()
        .h(px(ROW_HEIGHT))
        .flex()
        .flex_col()
        .justify_center()
        .px_3()
        .line_height(px(18.))
        .border_b_1()
        .border_color(rgb(0xe5e5e5))
        // Метка «не прочитано» — полоса слева, как в Outlook.
        .when(!r.seen, |d| d.child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(rgb(0x1a8cff))))
        .when(selected, |d| d.bg(rgb(0xcce4ff)))
        .hover(|d| d.bg(rgb(0xf0f6fd)))
        .cursor_pointer()
        .child(
            div()
                .flex()
                .gap_2()
                .child(div().flex_1().truncate().when(!r.seen, |d| d.font_weight(FontWeight::BOLD)).child(from))
                .child(div().text_color(rgb(0xcc3333)).child(icons))
                .child(div().text_sm().text_color(rgb(0x888888)).child(format_date(r.received_at))),
        )
        .child(div().text_sm().truncate().text_color(if r.seen { rgb(0x777777) } else { rgb(0x1a6fd1) }).child(line2))
        .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Не `super::*`: из `gpui_kit::*` пришёл бы атрибут `test` gpui вместо встроенного.
    use super::{EmailRow, More, Positions, ROW_HEIGHT, WINDOW_MARGIN, keyword_changes, reselect, rows_at};

    fn row(id: &str, seen: bool, flagged: bool) -> EmailRow {
        EmailRow {
            id: id.into(),
            thread_id: String::new(),
            mailbox_ids: Vec::new(),
            received_at: 0,
            subject: String::new(),
            from_name: String::new(),
            from_addr: String::new(),
            preview: String::new(),
            seen,
            flagged,
            has_attachment: false,
            size: 0,
        }
    }

    #[test]
    fn flags_become_keywords() {
        let r = row("a", false, false);
        assert_eq!(keyword_changes(&r, false, false), vec![]);
        assert_eq!(keyword_changes(&r, true, false), vec![("$seen", true)]);
        assert_eq!(keyword_changes(&r, true, true), vec![("$seen", true), ("$flagged", true)]);
        assert_eq!(keyword_changes(&row("a", true, true), true, false), vec![("$flagged", false)]);
    }

    #[test]
    fn each_folder_keeps_its_scroll_position() {
        let mut p = Positions::default();
        // Входящие прокручены на 10 строк вниз → Архив открывается сверху.
        let inbox = -10. * ROW_HEIGHT;
        assert_eq!(p.switch(Some("inbox"), inbox, Some("archive")), 0.);
        // Архив прокручен на 3 строки → во Входящих снова 10-я строка, в Архиве — 3-я.
        assert_eq!(p.switch(Some("archive"), -3. * ROW_HEIGHT, Some("inbox")), inbox);
        assert_eq!(p.switch(Some("inbox"), inbox, Some("archive")), -3. * ROW_HEIGHT);
        // Поиск — сверху и не запоминается; сброс поиска — назад в папку, где была прокрутка.
        assert_eq!(p.switch(Some("archive"), -3. * ROW_HEIGHT, None), 0.);
        assert_eq!(p.switch(None, -50. * ROW_HEIGHT, Some("archive")), -3. * ROW_HEIGHT);
        // Окно строк для первого чтения — там, куда прокручено.
        assert_eq!(rows_at(inbox, 30), 10..40);
        assert_eq!(rows_at(0., 30), 0..30);
    }

    #[test]
    fn more_is_asked_near_the_end_once() {
        // Демо и поиск: числа на сервере нет — не просить.
        let mut m = More::default();
        assert!(!m.wanted(500, 500));
        // В кэше 500 из 20 000: далеко от конца — рано, у конца — просить.
        m.set_total(Some(20_000));
        assert!(!m.wanted(30, 500));
        assert!(m.wanted(500 - WINDOW_MARGIN, 500));
        // Запрошено — второй раз не просить, пока нет ответа.
        m.asked = true;
        assert!(!m.wanted(500, 500));
        m.loaded(500);
        assert!(m.wanted(1000, 1000));
        // Всё загружено — не просить.
        assert!(!m.wanted(20_000, 20_000));
        // Пустая страница: больше нет; новые письма на сервере (число изменилось) — снова можно.
        m.loaded(0);
        assert!(!m.wanted(1000, 1000));
        m.set_total(Some(20_000));
        assert!(!m.wanted(1000, 1000), "the same total keeps the mailbox exhausted");
        m.set_total(Some(20_001));
        assert!(m.wanted(1000, 1000));
    }

    #[test]
    fn selection_follows_the_letter() {
        let rows = vec![row("new", false, false), row("a", true, false), row("b", true, false)];
        // Пришло новое письмо сверху: выбранное «a» сдвинулось на одну строку.
        assert_eq!(reselect(Some("a"), 0, &rows, 3), Some(1));
        assert_eq!(reselect(Some("b"), 100, &rows, 200), Some(102));
        // Письмо исчезло (удалено, перенесено) — выделения нет.
        assert_eq!(reselect(Some("gone"), 0, &rows, 3), None);
        assert_eq!(reselect(None, 0, &rows, 3), None);
    }
}
