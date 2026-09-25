# The TUI, recorded against the fake Graph: `demo/record.sh`.
Output docs/assets/demo-tui.gif
Source demo/common.tape

Type "mst"
Enter
Sleep 2s

# Through the views and folders: the sidebar opens what it's on.
Type "h"
Sleep 300ms
Type "g"
Sleep 900ms
Type "j"
Sleep 1.3s
Type@600ms "jjjj"
Sleep 1.3s
Type@600ms "jjj"
Sleep 1.3s

# A task's detail, then its due date in words, with the preview.
Type "l"
Sleep 500ms
Type@500ms "jj"
Sleep 1s
Type "e"
Sleep 800ms
Type "d"
Sleep 500ms
Ctrl+U
Type@110ms "in 2 days"
Sleep 1.8s
Enter
Sleep 1.3s

# Done, then undo.
Type "x"
Sleep 1.4s
Type "u"
Sleep 1.4s

# Filter.
Type "/"
Sleep 400ms
Type@120ms "tax"
Sleep 1.3s
Enter
Sleep 600ms
Escape
Sleep 800ms

# Themes: each one previews as you move; Enter keeps it.
Type ":"
Sleep 500ms
Type@120ms "theme"
Sleep 700ms
Enter
Sleep 900ms
Type "j"
Sleep 1.4s
Type@200ms "jjj"
Sleep 1.4s
Enter
Sleep 1.8s

Type "q"
Sleep 500ms
