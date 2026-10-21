# Conversation in the reading pane (S5-05).
thread-toggle = Conversation ({ $n })
thread-quotes-show = Show quotes
thread-quotes-hide = Hide quotes
thread-quotes-hidden = … quote hidden
thread-loading = loading the conversation…
thread-me = Me
thread-today = Today
thread-yesterday = Yesterday

# Conversation view: stack, in the list as in Outlook, folder menu (S5-05b).
thread-info = { $n ->
    [one] { $n } message
   *[other] { $n } messages
} · { $people ->
    [one] { $people } participant
   *[other] { $people } participants
}
thread-expand-all = Expand all
thread-collapse-all = Collapse all
thread-to = To: { $to }
thread-reply = Reply
thread-forward = Forward
thread-quote-show = show quote
thread-quote-hide = hide quote
thread-expand = Expand conversation
thread-collapse = Collapse conversation
thread-outlook = { $n ->
    [one] Conversation: { $n } message
   *[other] Conversation: { $n } messages
}
thread-prev = ← prev
thread-next = next →
thread-position = { $at } of { $n }
folder-menu-view = Conversation view
thread-view-feed = Feed
thread-view-stack = Stack
thread-view-outlook = As in Outlook
thread-view-default = Default
