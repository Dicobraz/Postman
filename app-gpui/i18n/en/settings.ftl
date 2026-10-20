# Mailbox settings on the server: automatic replies, mailbox usage (S4-05).
bar-vacation = Auto replies
vacation-title = Automatic replies
vacation-hint = The server sends the out-of-office reply, even when Postman is closed.
vacation-demo = Demo: automatic replies live on the server — an account is required.
vacation-loading = Reading settings from the server…
vacation-unsupported = The server does not support automatic replies.
vacation-enabled = Send automatic replies
vacation-period = Only during this time range
vacation-from = from
vacation-to = to
vacation-date-placeholder = DD.MM.YYYY HH:MM
vacation-subject = Subject
vacation-subject-placeholder = Out of office
vacation-body = Reply text
vacation-body-placeholder = I am on leave until …; for urgent matters contact …
vacation-save = Save
vacation-saving = Saving…
vacation-close = Close
vacation-saved = Automatic replies saved and turned on.
vacation-saved-off = Automatic replies are off.
vacation-failed = Automatic replies not saved: { $error }
vacation-bad-from = The “from” date is not recognized: DD.MM.YYYY or DD.MM.YYYY HH:MM.
vacation-bad-to = The “to” date is not recognized: DD.MM.YYYY or DD.MM.YYYY HH:MM.
vacation-bad-order = The “to” date is before the “from” date.
vacation-bar-on = Automatic replies are on: incoming mail gets an out-of-office reply.
vacation-bar-period = Automatic replies are on from { $from } to { $to }.
vacation-bar-from = Automatic replies are on from { $from }.
vacation-bar-to = Automatic replies are on until { $to }.
vacation-bar-edit = Settings
vacation-bar-off = Turn off
settings-quota = Mailbox: { $percent }% of { $unit ->
    [mb] { $n } MB
   *[gb] { $n } GB
}
settings-quota-warn = Mailbox is { $percent }% full: delete unneeded mail, or new mail will stop arriving.
