# Календарь (S3-05): неделя, подробности события, открытие занятости, помощник планирования.
cal-work-week = Рабочая неделя
cal-week-range = { $from } – { $to }
cal-prev-week = ‹ Пред.
cal-next-week = След. ›
cal-today = Сегодня
cal-new-meeting = ＋ Новая встреча (Ctrl+Shift+Q)
cal-assistant = 👥 Помощник планирования
cal-loading = Загрузка недели…
cal-me = Я
cal-day = { $d ->
    [0] Пн
    [1] Вт
    [2] Ср
    [3] Чт
    [4] Пт
    [5] Сб
   *[6] Вс
}
cal-minutes = { $n } мин
cal-free-for-all = Свободно у всех:
cal-no-slots = Общего окна на этой неделе нет.
cal-busy = занят
cal-tentative = под вопросом
cal-unavailable = нет на месте
cal-off-hours = нерабочее время
cal-no-data = нет данных
cal-failed = Календарь: { $error }

# Открыть свою занятость коллеге.
cal-share-label = Открыть мою занятость:
cal-share-placeholder = адрес коллеги
cal-share-button = Открыть
cal-share-note = Коллега видит вашу занятость, только если вы её открыли (без подробностей встреч). Пока коллега не открыл свою, в помощнике у него «нет данных», а не «свободен».
cal-share-bad = Не адрес: { $email }
cal-share-sending = Открываю занятость: { $email }…
cal-share-done = Ваша занятость открыта: { $email }

# Подробности события.
cal-details-when = Когда
cal-details-where = Где
cal-details-organizer = Организатор
cal-details-participants = Участники
cal-details-recurring = Повторяющееся событие
cal-details-close = Закрыть
cal-all-day = весь день
cal-optional = необязательный
cal-status-accepted = принял
cal-status-declined = отклонил
cal-status-tentative = под вопросом
cal-status-needs-action = нет ответа
cal-status-delegated = передал

# Окно встречи.
meeting-title-new = Новая встреча
meeting-window-failed = Окно встречи не открылось: { $error }
meeting-send = ➤ Отправить
meeting-subject = Тема
meeting-subject-placeholder = Тема встречи
meeting-location = Место
meeting-location-placeholder = Переговорная или ссылка
meeting-date = Дата
meeting-date-placeholder = дд.мм.гггг
meeting-start = Начало
meeting-start-placeholder = чч:мм
meeting-duration = Длительность
meeting-attendees = Участники
meeting-attendees-placeholder = Адреса через «;»
meeting-assistant = Помощник планирования
meeting-busy-loading = Запрашиваю занятость…
meeting-no-data-note = «нет данных» — коллега не открыл вам свою занятость или не найден на сервере; это не «свободен». Общие окна — по тем, чья занятость известна.
meeting-sending = Отправляю приглашения…
meeting-bad-time = Дата или время не распознаны: { $text }
meeting-bad-addresses = Не адреса: { $list }
meeting-no-server = Нужен сервер: в демо-режиме приглашения не отправляются.
meeting-failed = Не отправлено: { $error }
meeting-busy-failed = Занятость не получена: { $error }
meeting-sent = Приглашения отправлены: { $title }
meeting-created = Встреча создана: { $title }
meeting-untitled = (без темы)
