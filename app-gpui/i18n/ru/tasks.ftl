# Задачи (S6-05, Ctrl+4): списки, проекты, категории, список с группами, карточка задачи.
tasks-inbox = Входящие
tasks-today = Сегодня
tasks-upcoming = Предстоящие
tasks-from-mail = Из почты
tasks-someday = Когда-нибудь
tasks-done-list = Выполненные
tasks-projects = Проекты
tasks-categories = Категории
tasks-nav-note = Письмо со «Сроком» само становится задачей. Выполнили задачу — срок у письма снимается.
tasks-search = Поиск в задачах
tasks-quick-placeholder = Добавить задачу
tasks-new = Новая задача
tasks-complete = Выполнить
tasks-reopen = Вернуть в работу
tasks-edit = Изменить
tasks-delete = Удалить
tasks-pick = Выберите задачу слева.
tasks-count = { $n ->
    [one] { $n } задача
    [few] { $n } задачи
   *[many] { $n } задач
}
tasks-summary = { tasks-count }
tasks-summary-today = { $day } · { tasks-count }
tasks-summary-today-over = { $day } · { tasks-count } · { $over } просрочено
tasks-summary-mail = письма со сроком · { tasks-count }
tasks-summary-project = выполнено { $done } из { $total }
tasks-group-overdue = Просрочено
tasks-group-today = Сегодня
tasks-group-timed = ко времени
tasks-group-no-time = Без времени
tasks-group-tomorrow = Завтра
tasks-group-soon = Скоро
tasks-group-later = Позже
tasks-group-no-due = Без срока
tasks-group-yesterday = Вчера
tasks-group-earlier = Ранее
tasks-empty-inbox = Входящие пусты. Сюда попадают задачи без срока и проекта — разберите их, когда будет минута.
tasks-empty-today = На сегодня всё. Хорошего вечера.
tasks-empty-upcoming = Впереди пусто.
tasks-empty-mail = Писем со сроком нет. Поставьте «Срок» письму — оно появится здесь.
tasks-empty-someday = Здесь будут идеи и задачи без срока.
tasks-empty-done = Пока ничего не выполнено.
tasks-empty = Здесь задач нет.
tasks-kind-task = Задача
tasks-kind-mail = Из почты
tasks-letter = Письмо
tasks-open = Открыть
tasks-letter-gone = Письма больше нет в ящике.
tasks-field-due = Срок
tasks-field-remind = Напоминание
tasks-field-repeat = Повтор
tasks-field-project = Проект
tasks-field-category = Категория
tasks-due-at = { $day }, до { $time }
tasks-no-due = Без срока
tasks-no-remind = Нет
tasks-remind-set = { $n ->
    [one] { $n } напоминание
    [few] { $n } напоминания
   *[many] { $n } напоминаний
}
tasks-repeats = Повторяется
tasks-no-repeat = Не повторяется
tasks-no-project = Без проекта
tasks-no-category = Без категории
tasks-progress = { $done } из { $total }
tasks-subtasks = Подзадачи
tasks-no-subtasks = Подзадач нет
tasks-notes = Заметки
tasks-no-notes = Заметок нет
tasks-done = Выполнено · «{ $title }»
tasks-done-mail = Выполнено · срок у письма снят
tasks-reopened = Задача снова в работе
tasks-deleted = Задача удалена
tasks-deleted-mail = Удалено · срок у письма снят
tasks-status-mail = { $n ->
    [one] { $n } задача из писем
    [few] { $n } задачи из писем
   *[many] { $n } задач из писем
}
tasks-failed = Задача не сохранена на сервере: { $error }
