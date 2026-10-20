# Названия команд: ключ — `cmd-` + id команды (`mail.reply_all` → `cmd-mail-reply-all`), см. pets-commands.
cmd-mail-next = Следующее письмо
cmd-mail-prev = Предыдущее письмо
cmd-mail-first = Первое письмо
cmd-mail-last = Последнее письмо
cmd-mail-page-down = Страница вниз
cmd-mail-page-up = Страница вверх
cmd-mail-mark-read = Отметить как прочитанное
cmd-mail-mark-unread = Отметить как непрочитанное
cmd-mail-toggle-flag = Флаг
cmd-mail-new = Создать письмо
cmd-mail-reply = Ответить
cmd-mail-reply-all = Ответить всем
cmd-mail-forward = Переслать
cmd-mail-delete = Удалить
cmd-view-mail = Почта
cmd-view-calendar = Календарь
cmd-view-contacts = Контакты
cmd-search-focus = Поиск
cmd-search-clear = Сбросить поиск
cmd-calendar-new-meeting = Новая встреча (помощник планирования)
cmd-view-notifications = Уведомления

# Экран почты.
search-placeholder = Поиск (Ctrl+E), Enter — искать, Esc — сбросить
search-nothing = Ничего не найдено: «{ $query }»
folders-results = 🔍 Результаты ({ $n })
nav-mail = ✉ Почта
nav-calendar = 📅 Календарь
nav-contacts = 👥 Контакты
bar-new = ✉ Создать
bar-reply = ↩ Ответить
bar-reply-all = ↩↩ Ответить всем
bar-forward = ↪ Переслать
bar-delete = 🗑 Удалить
bar-flag = ⚑ Флаг
reading-empty = Выберите письмо
reading-from = От:
reading-to = Кому:
reading-date = Дата:
reading-remote-blocked = Внешние картинки не загружаются ({ $n }). Это защищает от отслеживания.
reading-image = [изображение]
reading-image-alt = [изображение: { $alt }]
reading-error = Письмо не показано: { $error }
sandbox-full = Песочница: полная
sandbox-partial = Песочница: частичная
sandbox-none = Песочница: нет
sandbox-weak = Песочница разбора писем ослаблена ({ $level }): письма открываются с меньшей защитой.
status-items = Элементов: { $n }
status-notifications = 🔔 { $n }
second-instance = Postman уже запущен — показано это окно.

# Календарь.
cal-work-week = Рабочая неделя
cal-assistant = 👥 Помощник планирования (Ctrl+Shift+Q)
cal-me = Я
cal-day = { $d ->
    [0] Пн
    [1] Вт
    [2] Ср
    [3] Чт
   *[4] Пт
}
cal-minutes = { $n } мин
cal-free-for-all = Свободно у всех:
cal-busy = занят
cal-tentative = под вопросом
cal-unavailable = нет на месте
cal-off-hours = нерабочее время

# Сбой запуска (журнал Windows и stderr).
fatal-data-dir = каталог данных
fatal-bench-mailbox = ящик для замера
fatal-demo-mailbox = демо-ящик
fatal-store = хранилище
fatal-window = окно
