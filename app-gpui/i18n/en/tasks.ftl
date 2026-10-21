# Tasks (S6-05, Ctrl+4): lists, projects, categories, grouped list, task card.
tasks-inbox = Inbox
tasks-today = Today
tasks-upcoming = Upcoming
tasks-from-mail = From mail
tasks-someday = Someday
tasks-done-list = Completed
tasks-projects = Projects
tasks-categories = Categories
tasks-nav-note = An email with a due date becomes a task by itself. Complete the task and the email's due date is cleared.
tasks-search = Search tasks
tasks-quick-placeholder = Add a task
tasks-new = New task
tasks-complete = Complete
tasks-reopen = Reopen
tasks-edit = Edit
tasks-delete = Delete
tasks-pick = Pick a task on the left.
tasks-count = { $n ->
    [one] { $n } task
   *[other] { $n } tasks
}
tasks-summary = { tasks-count }
tasks-summary-today = { $day } · { tasks-count }
tasks-summary-today-over = { $day } · { tasks-count } · { $over } overdue
tasks-summary-mail = emails with a due date · { tasks-count }
tasks-summary-project = { $done } of { $total } done
tasks-group-overdue = Overdue
tasks-group-today = Today
tasks-group-timed = by time
tasks-group-no-time = Any time
tasks-group-tomorrow = Tomorrow
tasks-group-soon = Soon
tasks-group-later = Later
tasks-group-no-due = No due date
tasks-group-yesterday = Yesterday
tasks-group-earlier = Earlier
tasks-empty-inbox = Inbox is empty. Tasks without a due date or project land here — sort them out when you have a minute.
tasks-empty-today = All done for today. Have a nice evening.
tasks-empty-upcoming = Nothing ahead.
tasks-empty-mail = No emails with a due date. Set a due date on an email and it shows up here.
tasks-empty-someday = Ideas and tasks without a due date go here.
tasks-empty-done = Nothing completed yet.
tasks-empty = No tasks here.
tasks-kind-task = Task
tasks-kind-mail = From mail
tasks-letter = Email
tasks-open = Open
tasks-letter-gone = The email is no longer in the mailbox.
tasks-field-due = Due
tasks-field-remind = Reminder
tasks-field-repeat = Repeat
tasks-field-project = Project
tasks-field-category = Category
tasks-due-at = { $day }, by { $time }
tasks-no-due = No due date
tasks-no-remind = None
tasks-remind-set = { $n ->
    [one] { $n } reminder
   *[other] { $n } reminders
}
tasks-repeats = Repeats
tasks-no-repeat = Does not repeat
tasks-no-project = No project
tasks-no-category = No category
tasks-progress = { $done } of { $total }
tasks-subtasks = Subtasks
tasks-no-subtasks = No subtasks
tasks-notes = Notes
tasks-no-notes = No notes
tasks-done = Done · “{ $title }”
tasks-done-mail = Done · the email's due date is cleared
tasks-reopened = The task is open again
tasks-deleted = Task deleted
tasks-deleted-mail = Deleted · the email's due date is cleared
tasks-failed = The task was not saved on the server: { $error }
