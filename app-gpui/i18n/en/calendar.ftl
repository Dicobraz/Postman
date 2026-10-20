# Calendar (S3-05): week, event details, sharing free/busy, scheduling assistant.
cal-work-week = Work week
cal-week-range = { $from } – { $to }
cal-prev-week = ‹ Prev
cal-next-week = Next ›
cal-today = Today
cal-new-meeting = ＋ New meeting (Ctrl+Shift+Q)
cal-assistant = 👥 Scheduling Assistant
cal-loading = Loading the week…
cal-me = Me
cal-day = { $d ->
    [0] Mon
    [1] Tue
    [2] Wed
    [3] Thu
    [4] Fri
    [5] Sat
   *[6] Sun
}
cal-minutes = { $n } min
cal-free-for-all = Free for everyone:
cal-no-slots = No common slot this week.
cal-busy = busy
cal-tentative = tentative
cal-unavailable = away
cal-off-hours = off hours
cal-no-data = no data
cal-failed = Calendar: { $error }

# Share my free/busy with a colleague.
cal-share-label = Share my free/busy:
cal-share-placeholder = colleague's address
cal-share-button = Share
cal-share-note = A colleague sees your free/busy only after you share it (no meeting details). Until a colleague shares theirs, the assistant shows "no data" for them, not "free".
cal-share-bad = Not an address: { $email }
cal-share-sending = Sharing free/busy with { $email }…
cal-share-done = Your free/busy is shared with { $email }

# Event details.
cal-details-when = When
cal-details-where = Where
cal-details-organizer = Organizer
cal-details-participants = Attendees
cal-details-recurring = Recurring event
cal-details-close = Close
cal-all-day = all day
cal-optional = optional
cal-status-accepted = accepted
cal-status-declined = declined
cal-status-tentative = tentative
cal-status-needs-action = no reply
cal-status-delegated = delegated

# Meeting window.
meeting-title-new = New meeting
meeting-window-failed = The meeting window did not open: { $error }
meeting-send = ➤ Send
meeting-subject = Subject
meeting-subject-placeholder = Meeting subject
meeting-location = Location
meeting-location-placeholder = Room or link
meeting-date = Date
meeting-date-placeholder = dd.mm.yyyy
meeting-start = Start
meeting-start-placeholder = hh:mm
meeting-duration = Duration
meeting-attendees = Attendees
meeting-attendees-placeholder = Addresses separated by ";"
meeting-assistant = Scheduling Assistant
meeting-busy-loading = Asking for free/busy…
meeting-no-data-note = "no data": the colleague has not shared free/busy with you or is not on the server; it does not mean "free". Common slots use those whose free/busy is known.
meeting-sending = Sending invitations…
meeting-bad-time = Date or time not recognized: { $text }
meeting-bad-addresses = Not addresses: { $list }
meeting-no-server = Needs a server: invitations are not sent in demo mode.
meeting-failed = Not sent: { $error }
meeting-busy-failed = Free/busy not received: { $error }
meeting-sent = Invitations sent: { $title }
meeting-created = Meeting created: { $title }
meeting-untitled = (no subject)
