# Command names: key is `cmd-` + command id (`mail.reply_all` → `cmd-mail-reply-all`), see pets-commands.
cmd-mail-next = Next message
cmd-mail-prev = Previous message
cmd-mail-first = First message
cmd-mail-last = Last message
cmd-mail-page-down = Page down
cmd-mail-page-up = Page up
cmd-mail-mark-read = Mark as read
cmd-mail-mark-unread = Mark as unread
cmd-mail-toggle-flag = Flag
cmd-mail-new = New message
cmd-mail-reply = Reply
cmd-mail-reply-all = Reply all
cmd-mail-forward = Forward
cmd-mail-delete = Delete
cmd-view-mail = Mail
cmd-view-calendar = Calendar
cmd-view-contacts = Contacts
cmd-search-focus = Search
cmd-search-clear = Clear search
cmd-calendar-new-meeting = New meeting (scheduling assistant)
cmd-view-notifications = Notifications

# Mail screen.
search-placeholder = Search (Ctrl+E), Enter to search, Esc to clear
search-nothing = Nothing found: “{ $query }”
folders-results = 🔍 Results ({ $n })
nav-mail = ✉ Mail
nav-calendar = 📅 Calendar
nav-contacts = 👥 Contacts
bar-new = ✉ New
bar-reply = ↩ Reply
bar-reply-all = ↩↩ Reply all
bar-forward = ↪ Forward
bar-delete = 🗑 Delete
bar-flag = ⚑ Flag
reading-empty = Select a message
reading-from = From:
reading-to = To:
reading-date = Date:
reading-remote-blocked = Remote images are not loaded ({ $n }). This protects you from tracking.
reading-image = [image]
reading-image-alt = [image: { $alt }]
reading-error = Message not shown: { $error }
sandbox-full = Sandbox: full
sandbox-partial = Sandbox: partial
sandbox-none = Sandbox: none
sandbox-weak = The message sandbox is weakened ({ $level }): messages open with less protection.
status-items = Items: { $n }
status-notifications = 🔔 { $n }
second-instance = Postman is already running — this window is shown.

# Calendar.
cal-work-week = Work week
cal-assistant = 👥 Scheduling Assistant (Ctrl+Shift+Q)
cal-me = Me
cal-day = { $d ->
    [0] Mon
    [1] Tue
    [2] Wed
    [3] Thu
   *[4] Fri
}
cal-minutes = { $n } min
cal-free-for-all = Free for everyone:
cal-busy = busy
cal-tentative = tentative
cal-unavailable = away
cal-off-hours = off hours

# Startup failure (Windows event log and stderr).
fatal-data-dir = data directory
fatal-bench-mailbox = bench mailbox
fatal-demo-mailbox = demo mailbox
fatal-store = store
fatal-window = window
