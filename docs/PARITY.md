# Соответствие classic Outlook

> Генерируется из реестра команд (`app-gpui/src/commands.rs`) тестом `parity_doc_is_current`.
> Руками не править. Обновить: `POSTMAN_UPDATE_DOCS=1 cargo test -p postman-gpui parity`.
> «есть» — у действия команды есть обработчик на экране; доля сделанного только растёт (тест).

Сделано: 36 из 36 (100 %).

| Функция | Команда | Хоткей | Статус |
|---|---|---|---|
| Следующее письмо | `mail.next` | `j`, `down` | есть |
| Предыдущее письмо | `mail.prev` | `k`, `up` | есть |
| Первое письмо | `mail.first` | `home` | есть |
| Последнее письмо | `mail.last` | `end` | есть |
| Страница вниз | `mail.page_down` | `pagedown` | есть |
| Страница вверх | `mail.page_up` | `pageup` | есть |
| Отметить как прочитанное | `mail.mark_read` | `ctrl-q` | есть |
| Отметить как непрочитанное | `mail.mark_unread` | `ctrl-u` | есть |
| Флаг | `mail.toggle_flag` | `insert` | есть |
| Создать письмо | `mail.new` | `ctrl-n` | есть |
| Ответить | `mail.reply` | `ctrl-r` | есть |
| Ответить всем | `mail.reply_all` | `ctrl-shift-r` | есть |
| Переслать | `mail.forward` | `ctrl-f` | есть |
| Удалить | `mail.delete` | `delete` | есть |
| Беседа: лента вместо письма | `thread.toggle` |  | есть |
| Беседа: показать цитаты | `thread.quotes` |  | есть |
| Следующее письмо беседы | `thread.next` | `ctrl-right` | есть |
| Предыдущее письмо беседы | `thread.prev` | `ctrl-left` | есть |
| Почта | `view.mail` | `ctrl-1` | есть |
| Календарь | `view.calendar` | `ctrl-2` | есть |
| Контакты | `view.contacts` | `ctrl-3` | есть |
| Уведомления | `view.notifications` |  | есть |
| Поиск | `search.focus` | `ctrl-e` | есть |
| Сбросить поиск | `search.clear` | `escape` | есть |
| Новая встреча (помощник планирования) | `calendar.new_meeting` | `ctrl-shift-q` | есть |
| Предыдущая неделя | `calendar.prev_week` | `alt-up` | есть |
| Следующая неделя | `calendar.next_week` | `alt-down` | есть |
| Сегодня в календаре | `calendar.today` |  | есть |
| Отправить письмо | `compose.send` | `ctrl-enter`, `alt-s` | есть |
| Вложить файл | `compose.attach` |  | есть |
| Предпросмотр письма | `compose.preview` | `ctrl-shift-p` | есть |
| Закрыть окно письма | `compose.close` | `escape` | есть |
| Отправить приглашения | `meeting.send` | `ctrl-enter`, `alt-s` | есть |
| Закрыть окно встречи | `meeting.close` | `escape` | есть |
| Автоматические ответы | `settings.vacation` |  | есть |
| Правила | `settings.rules` |  | есть |
