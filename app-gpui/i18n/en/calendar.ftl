# Calendar (S3-05, look — S5-11): week, event details, sharing free/busy.
cal-work-week = Work week
cal-today = Today
cal-today-tip = Go to today (Alt+Home)
cal-prev-week-tip = Previous week (Alt+↑)
cal-next-week-tip = Next week (Alt+↓)
cal-new-meeting = New meeting
cal-new-meeting-tip = New meeting
cal-loading = Loading the week…
cal-day = { $d ->
    [0] Mon
    [1] Tue
    [2] Wed
    [3] Thu
    [4] Fri
    [5] Sat
   *[6] Sun
}
cal-weekday = { $d ->
    [0] Monday
    [1] Tuesday
    [2] Wednesday
    [3] Thursday
    [4] Friday
    [5] Saturday
   *[6] Sunday
}
cal-month = { $m ->
    [1] January
    [2] February
    [3] March
    [4] April
    [5] May
    [6] June
    [7] July
    [8] August
    [9] September
    [10] October
    [11] November
   *[12] December
}
cal-month-of = { $m ->
    [1] January
    [2] February
    [3] March
    [4] April
    [5] May
    [6] June
    [7] July
    [8] August
    [9] September
    [10] October
    [11] November
   *[12] December
}
cal-month-title = { $month } { $year }
cal-long-day = { $wd }, { $month } { $day }
cal-minutes = { $n } min
cal-hours = { $n } h
cal-busy = busy
cal-tentative = tentative
cal-unavailable = out of office
cal-failed = Calendar: { $error }

# Left: my calendars, categories, free/busy.
cal-my-calendar = My calendar
cal-group-mine = My calendars
cal-group-categories = Categories
cal-group-share = Free/busy
cal-share-open = Share my free/busy
cal-share-label = Share my free/busy with a colleague
cal-share-placeholder = colleague's address
cal-share-button = Share
cal-share-cancel = Cancel
cal-share-note = A colleague sees your free/busy only if you shared it (without meeting details). Until a colleague shares theirs, the assistant shows "no data" for them, not "free".
cal-share-bad = Not an address: { $email }
cal-share-sending = Sharing free/busy: { $email }…
cal-share-done = Your free/busy is shared: { $email }

# Week.
cal-all-day-1 = All
cal-all-day-2 = day
cal-all-day = all day
cal-waits-answer = awaiting reply

# Event details.
cal-details-empty = Pick a meeting to see its details here.
cal-details-recurring = Recurring event
cal-chip-organizer = You organize
cal-chip-waits = Awaiting reply
cal-chip-tentative = Tentative
cal-chip-declined = Declined
cal-chip-accepted = Accepted
cal-invited-by = { $name } invites you
cal-accept = Accept
cal-tentative-answer = Tentative
cal-decline = Decline
cal-answered-accepted = You accepted — the organizer is notified
cal-answered-tentative = You answered "Tentative" — the organizer is notified
cal-answered-declined = You declined — the event is hidden from the grid
cal-change-answer = Change reply
cal-conflict = Overlaps "{ $title }", { $time }
cal-conflict-more = and { $n } more
cal-participants-n = Attendees · { $n }
cal-sum-accepted = accepted: { $n }
cal-sum-tentative = tentative: { $n }
cal-sum-declined = declined: { $n }
cal-sum-none = no reply: { $n }
cal-you = { $name } (you)
cal-organizer = organizer
cal-optional = optional
cal-external = External
cal-write-participants = Email attendees

# Meeting tab.
meeting-title-new = New meeting
meeting-tab-titled = New meeting · { $title }
meeting-send = Send
meeting-send-tip = Send invitations
meeting-subject = Subject
meeting-subject-placeholder = Meeting subject
meeting-required = Required
meeting-optional = Optional
meeting-rooms = Room
meeting-location = Location / link
meeting-location-placeholder = Room or link
meeting-when = Start – end
meeting-date-placeholder = dd.mm.yyyy
meeting-start-placeholder = hh:mm
meeting-duration = Duration
meeting-description = Description
meeting-desc-add = Add description
meeting-desc-hide = Collapse description
meeting-desc-hint = agenda, materials, links
meeting-description-placeholder = Meeting agenda
meeting-attendees-placeholder = Address or name — Enter or ";"
meeting-external-short = external
meeting-presence-tip = { $name } — { $state } at the picked time
meeting-presence-free = free
meeting-presence-unknown = free/busy unknown
meeting-presence-external = external, free/busy unknown
meeting-me = You
meeting-role-organizer = Organizer
meeting-role-required = Required
meeting-role-optional = Optional
meeting-role-room = Room
meeting-not-counted = { $role } · not counted
meeting-only-free = Common free time only
meeting-with-optional = Count optional attendees
meeting-legend-free = Everyone free
meeting-legend-busy = Busy
meeting-legend-unknown = Unknown
meeting-legend-external = External
meeting-not-counted-n = Not counted: { $n } — free/busy unknown
meeting-participants-n = Attendees · { $n }
meeting-everyone = All attendees
meeting-known = { $known } of { $n } with free/busy data
meeting-add-person = Add attendee
meeting-row-hidden = Calendar not shared with you — "no data", not "free"
meeting-row-external = External address — the invitation goes by email
meeting-external-note = External attendees get the invitation by email
meeting-busy-loading = Requesting free/busy…
meeting-busy-failed = Free/busy not received: { $error }
meeting-sending = Sending invitations…
meeting-bad-time = Date or time not recognized: { $text }
meeting-bad-addresses = Not addresses: { $list }
meeting-no-server = A server is needed: invitations are not sent in demo mode.
meeting-failed = Not sent: { $error }
meeting-sent = Invitations sent: { $title }
meeting-created = Meeting created: { $title }
meeting-untitled = (no subject)

# Invitation in a letter (card, S5-11).
invite-day = { $wd }, { $month } { $day }
invite-month-short = { $m ->
    [1] Jan
    [2] Feb
    [3] Mar
    [4] Apr
    [5] May
    [6] Jun
    [7] Jul
    [8] Aug
    [9] Sep
    [10] Oct
    [11] Nov
   *[12] Dec
}
invite-organizer = Organizer: { $name }
invite-accepted-of = { $n } of { $total } accepted
invite-updated = The organizer changed: { $what }
invite-changed-time = time
invite-changed-place = location
invite-changed-title = subject
invite-done-accepted = You accepted
invite-done-tentative = You answered "Tentative"
invite-done-declined = You declined
invite-route = The reply goes through the server calendar · from invite.ics, parsed in the sandbox
invite-route-done = The reply is stored in the server calendar by the event UID · from invite.ics, parsed in the sandbox
invite-cancelled = Meeting cancelled by the organizer · { $organizer }
invite-outdated-newer = Outdated invitation: the calendar has a newer version of the meeting
invite-outdated-past = The meeting is over
invite-your-day = Your day · { $day }
invite-day-loading = Loading the calendar…
invite-conflict = Overlaps: { $title } { $time }
invite-conflict-many = Overlaps: { $title } { $time } and { $n } more
invite-no-conflict = No overlaps
invite-sample-old-place = Room 2
